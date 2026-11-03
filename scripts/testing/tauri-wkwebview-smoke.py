#!/usr/bin/env python3
"""Actual packaged macOS WKWebView smoke (run on a dedicated GUI CI runner).

Uses production Tauri modules and the shared Linux smoke JS/report fixtures;
only the temporary composition root gains read-only test instrumentation.
--prepare-only performs source generation on any platform, never runtime proof.
No production files, existing daemon, listener, release or external service is changed.
"""
import argparse
import contextlib
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

_shared_spec = importlib.util.spec_from_file_location('splitty_linux_webkit_fixture', Path(__file__).with_name('tauri-webkit-smoke.py'))
_shared = importlib.util.module_from_spec(_shared_spec)
_shared_spec.loader.exec_module(_shared)
replace_once, rewrite, observe_function = _shared.replace_once, _shared.rewrite, _shared.observe_function


def parse_processes(text):
    result = {}
    for line in text.splitlines():
        fields = line.strip().split(None, 2)
        if len(fields) == 3:
            result[int(fields[0])] = {'parent':int(fields[1]), 'command':fields[2]}
    return result


def process_table():
    return parse_processes(subprocess.check_output(['/bin/ps','-axo','pid=,ppid=,command='], text=True))


def tree(root, table):
    found = {root}
    while True:
        extended = found | {pid for pid, row in table.items() if row['parent'] in found}
        if extended == found: return found
        found = extended


def descendants(root):
    return tree(root, process_table())


class ProcBsdInfo(ctypes.Structure):
    # Darwin <sys/proc_info.h>, PROC_PIDTBSDINFO. Start time includes usec;
    # never kill an unrelated recycled PID based on ps's seconds-only lstart.
    _fields_ = [(name, ctypes.c_uint32) for name in
        ['flags','status','xstatus','pid','ppid','uid','gid','ruid','rgid','svuid','svgid','rfu1']]
    _fields_ += [('comm',ctypes.c_char*16),('name',ctypes.c_char*32)]
    _fields_ += [(name,ctypes.c_uint32) for name in ['nfiles','pgid','pjobc','e_tdev','e_tpgid']]
    _fields_ += [('nice',ctypes.c_int32),('start_sec',ctypes.c_uint64),('start_usec',ctypes.c_uint64)]


def process_identity(pid):
    lib = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
    query = lib.proc_pidinfo
    query.argtypes = [ctypes.c_int,ctypes.c_int,ctypes.c_uint64,ctypes.c_void_p,ctypes.c_int]
    query.restype = ctypes.c_int
    info = ProcBsdInfo()
    size = query(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
    if size == 0: return None
    if size != ctypes.sizeof(info): raise RuntimeError('unexpected Darwin proc_bsdinfo layout')
    return {'pid':pid,'birth':[info.start_sec,info.start_usec],'state':info.status}


def identity_alive(saved):
    current = saved and process_identity(saved['pid'])
    return bool(current and current['birth'] == saved['birth'] and current['state'] != 5)  # SZOMB


def parse_listeners(text):
    result, pid, command = [], None, None
    for line in text.splitlines():
        if line.startswith('p'): pid = int(line[1:])
        elif line.startswith('c'): command = line[1:]
        elif line.startswith('n'): result.append({'pid':pid,'command':command,'address':line[1:]})
    return result


def listeners(pids):
    if not pids: return []
    query = subprocess.run(['/usr/sbin/lsof','-nP','-a','-p',','.join(map(str,sorted(pids))),
        '-iTCP','-sTCP:LISTEN','-Fpcn'], text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if query.returncode not in [0,1] or query.stderr.strip():
        raise RuntimeError(f'lsof inventory failed: {query.stderr}')
    return parse_listeners(query.stdout)


# Reuse the daemon fixture's CLI, preservation and exact-identity cleanup;
# replace its OS observers rather than substituting application behavior.
_shared.process_identity, _shared.identity_alive, _shared.descendants = process_identity, identity_alive, descendants


class IndependentDaemon(_shared.IndependentDaemon):
    def __enter__(self):
        try:
            self.started = True
            self.cli(['daemon','start'])
            self.job = json.loads(self.cli(['run','--json','--','/bin/sh']))
            self.discovery = (self.config/'daemon.json').read_bytes()
            self.info = json.loads(self.discovery)
            self.daemon = process_identity(self.info['pid'])
            self.identities = [self.daemon]
            table = process_table()
            self.keeper_pids = [pid for pid in tree(self.info['pid'],table)
                if 'session-keeper' in table.get(pid,{}).get('command','')]
            self.identities += [process_identity(pid) for pid in self.keeper_pids]
            self.keeper_sockets = self.keepers()
            if not self.keeper_pids or not self.keeper_sockets: raise RuntimeError('fixture keeper not observed')
            self.pid_file = self.home/'independent-job.pid'
            self.cli(['input',self.job['jobId'],'--json'], f"printf '%s\n' \"$$\" > '{self.pid_file}'\n")
            deadline = time.monotonic()+5
            while not self.pid_file.exists() and time.monotonic()<deadline: time.sleep(.02)
            self.shell = process_identity(int(self.pid_file.read_text()))
            self.identities.append(self.shell)
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise


def macos_report(source):
    # Keep all shared protocol/quota/bootstrap assertions unchanged. AppKit's
    # windows array detects actual orphan NSWindows beyond Tauri's registry.
    start = source.index('        use gtk::prelude::*;')
    end = source.index('        let _ = send.send',start)
    return source[:start]+"""        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        let mut managed: Vec<_> = observed.webview_windows().keys().cloned().collect();
        managed.sort();
        let application = NSApplication::sharedApplication(
            MainThreadMarker::new().expect("AppKit inventory runs on the actual main thread"));
        let visible = application.windows().iter().filter(|window| window.isVisible()).count();
"""+source[end:]


def run_macos(repo, args, evidence, env):
    (evidence/'platform.json').write_text(json.dumps({
        'runtime':'actual Tauri/Wry WKWebView','os':subprocess.check_output(['/usr/sbin/sysctl','-n','kern.osrelease'],text=True).strip(),
        'scope':'owned descendants plus newly observed WebKit XPC candidates; XPC attribution is observational; pre-existing shared XPC helpers are not attributed',
        'inboundFrameworkAllocationBound':False,'genericCommandReplyRssBound':False,
    },indent=2))
    with tempfile.TemporaryDirectory(prefix='splitty-wk-home-', dir='/tmp') as private:
        home = Path(private)
        env = {key:value for key,value in env.items() if not key.startswith('SPLITTY_')}
        env.pop('ZDOTDIR',None)
        env.update(HOME=str(home),XDG_CONFIG_HOME=str(home/'config'),XDG_DATA_HOME=str(home/'data'),
            XDG_CACHE_HOME=str(home/'cache'),SPLITTY_CONFIG_DIR=str(home/'splitty'),SPLITTY_SMOKE_EVIDENCE=str(evidence))
        with contextlib.ExitStack() as cleanup:
            daemon = cleanup.enter_context(IndependentDaemon(repo/'target/debug/splitty',env,home,evidence)) if args.coexist_daemon else None
            baseline = process_table()
            with (evidence/'runtime.log').open('w') as log:
                child = subprocess.Popen([str(repo/'target/debug/splitty-wkwebview-runtime-smoke')],env=env,
                    stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
                samples, shells, observed, candidate_xpc, terminal_identities = [], {}, {}, {}, {}
                count = 0
                try:
                    deadline = time.monotonic()+90+6*args.reloads
                    while child.poll() is None and time.monotonic()<deadline:
                        table = process_table()
                        owned = tree(child.pid,table)
                        # WKWebView's XPC helpers can have launchd as parent.
                        candidates = {pid for pid,row in table.items() if pid not in baseline and 'com.apple.WebKit.' in row['command']}
                        for pid in candidates: candidate_xpc[pid] = table[pid]['command']
                        samples += listeners(owned | candidates)
                        count += 1
                        for pid in owned:
                            row = table.get(pid)
                            if not row: continue
                            observed[pid] = row['command']
                            command = Path(row['command'].split()[0]).name.lstrip('-')
                            if command in ['bash','zsh','sh','dash','fish']:
                                identity = process_identity(pid)
                                if identity: shells[pid] = identity
                        for report in ['first.json','epochs.json']:
                            try: values = json.loads((evidence/report).read_text())
                            except (OSError,ValueError): continue
                            for value in values if isinstance(values,list) else [values]:
                                pid = value.get('terminalPid')
                                if isinstance(pid,int) and pid not in terminal_identities:
                                    identity = process_identity(pid)
                                    if identity: terminal_identities[pid] = identity
                        time.sleep(.1)
                    if child.poll() is None: raise RuntimeError('actual WKWebView smoke timed out')
                    result_path = evidence/'result.json'
                    result = json.loads(result_path.read_text()) if result_path.exists() else {'error':'app exited without report'}
                    deadline = time.monotonic()+5
                    while any(identity_alive(saved) for saved in shells.values()) and time.monotonic()<deadline: time.sleep(.05)
                    terminal_pids = [value.get('terminalPid') for value in [result.get('first',{}),result.get('reloaded',{})]]
                    terminal_exited = all(isinstance(pid,int) and not identity_alive(terminal_identities.get(pid) or process_identity(pid)) for pid in terminal_pids)
                    result.update(platform='macOS WKWebView', runtimeExecuted=True, mode='full-ui' if args.full_ui else 'minimal',
                        tcpListeners=samples,listenerSamples=count,observedProcesses=observed,webKitXpcCandidates=candidate_xpc,
                        ownedShellPids=sorted(shells),ownedShellPidsExited=not any(identity_alive(saved) for saved in shells.values()),
                        terminalPidsExited=terminal_exited,appExitCode=child.returncode,
                        packagedAssets={str(path.relative_to(args.harness_dir/'assets')):hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in (args.harness_dir/'assets').rglob('*') if path.is_file()})
                    if daemon: result['coexistence'] = daemon.after_app(result)
                    result_path.write_text(json.dumps(result,indent=2))
                    print(json.dumps(result,indent=2))
                    if child.returncode or not result.get('success') or samples or not terminal_exited or not result['ownedShellPidsExited'] or (daemon and not result['coexistence']['success']):
                        raise RuntimeError(f'actual WKWebView smoke failed; see {evidence}')
                finally:
                    if child.poll() is None:
                        os.killpg(child.pid,signal.SIGTERM)
                        try: child.wait(timeout=5)
                        except subprocess.TimeoutExpired: os.killpg(child.pid,signal.SIGKILL); child.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence-dir', type=Path)
    parser.add_argument('--prepare-only', action='store_true', help='generate temporary macOS harness without building or running it')
    parser.add_argument('--reloads', type=int, default=1, help='actual document reloads (1..64); full UI checks heartbeats in first and last documents')
    parser.add_argument('--coexist-daemon', action='store_true', help='prove app lifetime is independent from a real fixture daemon/keeper job')
    parser.add_argument('--full-ui', action='store_true', help='package frontend/dist and exercise the production Svelte application/assets')
    parser.add_argument('--harness-dir', type=Path, default=Path('/tmp/splitty-wkwebview-runtime-smoke'))
    args = parser.parse_args()
    if not 1 <= args.reloads <= 64: parser.error('--reloads must be between 1 and 64')
    repo = Path(__file__).resolve().parents[2]
    harness = args.harness_dir.resolve()
    if sys.platform != 'darwin' and not args.prepare_only:
        raise RuntimeError('actual WKWebView execution requires macOS; --prepare-only is portable')
    evidence = (args.evidence_dir or Path(tempfile.mkdtemp(prefix='splitty-webkit-evidence-'))).resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    for name in ['first.json','result.json','events.json','epochs.json','coexistence.json','daemon-cleanup.json']:
        (evidence/name).unlink(missing_ok=True)
    (harness / 'src').mkdir(parents=True, exist_ok=True)
    if (harness / 'assets').exists(): shutil.rmtree(harness / 'assets')
    (harness / 'assets').mkdir(exist_ok=True)
    if args.full_ui:
        if not (repo/'frontend/dist/index.html').exists(): raise RuntimeError('build frontend/dist before --full-ui')
        shutil.copytree(repo/'frontend/dist', harness/'assets', dirs_exist_ok=True)
    else: (harness / 'assets/index.html').write_text('<!doctype html><html><head><title>Splitty native WebKit smoke</title></head><body>Native IPC runtime fixture</body></html>')
    production_config = json.loads((repo/'crates/splitty-tauri/tauri.conf.json').read_text())
    version = production_config['version']
    package = {
        'productName':'Splitty WebKit Smoke', 'version':version, 'identifier':'com.splitty.runtime-smoke',
        'build': {'frontendDist':'assets'},
        'app': {'windows':[], 'security':production_config['app']['security']},
        'bundle': {'active':False, 'icon':[str(repo/'crates/splitty-tauri/icons/128x128.png')]},
    }
    (harness/'tauri.conf.json').write_text(json.dumps(package))
    dependencies = '\n'.join(f'{name} = {{ path = {json.dumps(str(repo/"crates"/name))} }}' for name in ['splitty-core','splitty-server','splitty-remote','splitty-transport'])
    (harness/'Cargo.toml').write_text('''[package]
name = "splitty-wkwebview-runtime-smoke"
version = "0.1.30"
edition = "2021"
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
tokio = { version = "1", features = ["full"] }
rand = "0.8"
subtle = "2"
hex = "0.4"
base64 = "0.22"
objc2 = "0.6"
objc2-app-kit = { version = "0.3", default-features = false, features = ["std", "NSApplication", "NSWindow", "NSResponder"] }
axum = { version = "0.8", features = ["ws"] }
''' .replace('version = "0.1.30"', 'version = '+json.dumps(version), 1) + dependencies + '\n[build-dependencies]\ntauri-build = "2"\n')
    shutil.copyfile(repo/'Cargo.lock', harness/'Cargo.lock')
    shutil.copyfile(repo/'crates/splitty-tauri/build.rs', harness/'build.rs')
    build = (harness/'build.rs').read_text().replace('"native_bootstrap_begin",','"smoke_report", "smoke_windows",\n            "native_bootstrap_begin",',1)
    (harness/'build.rs').write_text(build)
    source = (repo/'crates/splitty-tauri/src/lib.rs').read_text()
    for name in ['capability','client_ipc','downloads','editor','login_path','navigation','remote','windows_wsl']:
        link = harness/'src'/(name+'.rs')
        if link.exists() or link.is_symlink(): link.unlink()
        link.symlink_to(repo/'crates/splitty-tauri/src'/(name+'.rs'))
    # Expose the existing test-only counter in the temporary module copy.
    # This adds read-only instrumentation, without changing broker behavior.
    broker_source = (repo/'crates/splitty-tauri/src/client_ipc.rs').read_text()
    broker_source = replace_once(broker_source, '#[cfg(test)]\n    fn counts(&self)', 'pub(crate) fn counts(&self)')
    (harness/'src/client_ipc.rs').unlink()
    broker_source += '''
impl Broker { pub(crate) fn smoke_stale_handles(&self) -> std::collections::HashSet<Handle> {
        let registry = self.registry.lock().unwrap();
        registry.entries.keys().copied().filter(|handle| !registry.current(*handle)).collect()
    } }
'''
    (harness/'src/client_ipc.rs').write_text(broker_source)
    link = harness/'src/client_ipc'
    if link.is_symlink(): link.unlink()
    elif link.exists(): shutil.rmtree(link)
    link.mkdir()
    for original in (repo/'crates/splitty-tauri/src/client_ipc').iterdir():
        if original.name == 'carrier.rs':
            (link/original.name).write_text(original.read_text()+'''
impl Native { pub(crate) fn smoke_delivery_handles(&self) -> std::collections::HashSet<Handle> {
                self.deliveries.lock().unwrap().keys().copied().collect()
            } }
''')
        else: (link/original.name).symlink_to(original)
    icons = harness/'icons'
    if not icons.is_symlink(): icons.symlink_to(repo/'crates/splitty-tauri/icons', target_is_directory=True)
    source = rewrite(source, r'native_bootstrap_begin\s*,\s*native_bootstrap_confirm\s*,',
                     'smoke_report, smoke_windows, native_bootstrap_begin, native_bootstrap_confirm,', count=2)
    source = rewrite(source, r'\.on_navigation\(move \|url\| \{',
                     '.on_navigation(move |url| { smoke_event(&format!("navigation:{}",url));')
    source = replace_once(source, '.initialization_script(script)', '.initialization_script(script).initialization_script(include_str!("smoke.js"))')
    source = observe_function(source, 'native_bootstrap_begin', 'smoke_event("begin");')
    source = rewrite(source, r'(let\s+\(proof,\s*policy\)\s*=\s*native\s*\.broker\s*\.bootstrap_confirm[\s\S]*?\?;)',
                     r'\1\nsmoke_event("confirm"); smoke_proofs().lock().unwrap().push(proof.clone());')
    for event in ['Started','Finished']:
        source = rewrite(source, r'(tauri::webview::PageLoadEvent::'+event+r'\s*=>\s*\{)',
                         r'\1 smoke_event("'+event.lower()+r'");')
    source = source.replace('app.add_capability(capability::main_window())?;',
             'app.add_capability(capability::main_window())?;\napp.add_capability(tauri::ipc::CapabilityBuilder::new("smoke-report").window("main").permission("allow-smoke-report").permission("allow-smoke-windows"))?;')
    source = rewrite(source, r'app\.run\(\|app_handle, event\| \{', 'app.run(|app_handle, event| { if let tauri::RunEvent::ExitRequested { code: None, api, .. } = &event { api.prevent_exit(); }', count=2)
    source += '\n' + macos_report((repo/'scripts/testing/tauri-webkit-smoke/report.rs').read_text())
    source += '\nfn main() { run(); }\n'
    (harness/'src/main.rs').write_text(source)
    source_files = ['crates/splitty-tauri/Cargo.toml', 'crates/splitty-tauri/tauri.conf.json',
                    'crates/splitty-tauri/build.rs', 'crates/splitty-tauri/src/lib.rs', 'crates/splitty-tauri/src/navigation.rs',
                    'crates/splitty-tauri/src/client_ipc.rs', 'crates/splitty-tauri/src/client_ipc/bootstrap.js',
                    'crates/splitty-tauri/src/client_ipc/carrier.rs', 'crates/splitty-tauri/src/client_ipc/provider.rs',
                    'frontend/src/lib/protocol/native_socket.ts', 'frontend/src/lib/protocol/tauri_channel.ts']
    source_files += ['scripts/testing/tauri-wkwebview-smoke.py']
    source_files += [str(path.relative_to(repo)) for path in (repo/'scripts/testing/tauri-webkit-smoke').rglob('*') if path.is_file()]
    (evidence/'sources.json').write_text(json.dumps({name:hashlib.sha256((repo/name).read_bytes()).hexdigest() for name in source_files}, indent=2))
    vite = repo/'frontend/node_modules/vite/dist/node/index.js'
    config = f'''import {{ defineConfig }} from {json.dumps(str(vite))};
export default defineConfig({{ build: {{ outDir: {json.dumps(str(harness/'src'))}, emptyOutDir:false, minify:false,
lib: {{ entry: {json.dumps(str(repo/'scripts/testing/tauri-webkit-smoke/entry.ts'))}, name:'SplittyRuntimeSmoke', formats:['iife'], fileName:()=> 'smoke.js' }}, target:'es2022' }}, define: {{ __SPLITTY_FULL_UI__: {str(args.full_ui).lower()}, __SPLITTY_SMOKE_RELOADS__: {args.reloads} }} }});
'''
    (harness/'vite.config.mjs').write_text(config)
    env = dict(os.environ, CARGO_TARGET_DIR=str(repo/'target'))
    if args.prepare_only:
        (evidence/'preparation.json').write_text(json.dumps({'runtimeVerified':False,'platform':'macOS WKWebView','harness':str(harness),'reason':'source preparation only'},indent=2))
        print(f'Prepared {harness}; actual macOS runtime UNVERIFIED')
        return
    for command, logfile in [([str(repo/'frontend/node_modules/.bin/vite'),'build','--config',str(harness/'vite.config.mjs')], 'bundle.log'),
                             (['cargo','build','--manifest-path',str(harness/'Cargo.toml')], 'build.log')]:
        with (evidence/logfile).open('w') as log:
            subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    run_macos(repo, args, evidence, env)


if __name__ == '__main__':
    main()
