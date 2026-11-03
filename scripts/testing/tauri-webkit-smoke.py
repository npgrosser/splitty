#!/usr/bin/env python3
"""Run production Tauri/Wry modules in an isolated, packaged Linux WebKit smoke.

No mock runtime, browser driver, HTTP UI server, production patch or system
package install is used. Test-only instrumentation is added to temporary source copies, including
read-only native counters; transport and application flows remain production code. Requires Xvfb and the repository's installed frontend dependencies.
Every GUI run uses an isolated display; the user desktop is never used.
"""
import argparse
import contextlib
import json
import hashlib
import importlib.util
import sys
import os
import re
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time


_isolation_spec = importlib.util.spec_from_file_location('splitty_unix_isolation_fixture', Path(__file__).with_name('tauri-webkit-smoke')/'isolation_fixture.py')
_isolation = importlib.util.module_from_spec(_isolation_spec)
_isolation_spec.loader.exec_module(_isolation)

def replace_once(source, before, after):
    if source.count(before) != 1:
        raise RuntimeError(f"production instrumentation anchor changed: {before!r}")
    return source.replace(before, after, 1)


def rewrite(source, pattern, replacement, count=1):
    result, matches = re.subn(pattern, replacement, source)
    if matches != count:
        raise RuntimeError(f"production instrumentation anchor changed: {pattern!r}, matches={matches}")
    return result


def observe_function(source, name, statement):
    index = source.index('fn '+name+'(')
    body = source.index('{', index)+1
    return source[:body]+'\n'+statement+'\n'+source[body:]


def descendants(root):
    found = {root}
    changed = True
    while changed:
        changed = False
        for stat in Path('/proc').glob('[0-9]*/stat'):
            try:
                fields = stat.read_text().rsplit(')', 1)[1].split()
                pid, parent = int(stat.parent.name), int(fields[1])
                if parent in found and pid not in found:
                    found.add(pid)
                    changed = True
            except (OSError, ValueError, IndexError):
                pass
    return found


def listeners(root):
    sockets = set()
    network_views = {}
    for pid in descendants(root):
        try: network_views[os.readlink(f'/proc/{pid}/ns/net')] = pid
        except OSError: pass
        try:
            for fd in Path(f'/proc/{pid}/fd').iterdir():
                try:
                    link = os.readlink(fd)
                    if link.startswith('socket:['):
                        sockets.add(link[8:-1])
                except OSError:
                    pass
        except OSError:
            pass
    result = []
    # Inspect every observed network namespace, including WebKit's sandbox
    # view, rather than assume /proc/net describes the whole process subtree.
    for namespace, pid in network_views.items():
        for name in ['tcp', 'tcp6']:
            try: rows = Path(f'/proc/{pid}/net/{name}').read_text().splitlines()[1:]
            except OSError: continue
            for row in rows:
                fields = row.split()
                if fields[3] == '0A' and fields[9] in sockets:
                    result.append({'namespace': namespace, 'family': name, 'address': fields[1], 'inode': fields[9]})
    return result


def process_identity(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return {'pid': pid, 'birth': fields[19], 'state': fields[0]}
    except (OSError, IndexError):
        return None


def identity_alive(saved):
    current = saved and process_identity(saved['pid'])
    return bool(current and current['birth'] == saved['birth'] and current['state'] not in ['Z', 'X'])


class IsolatedDisplay:
    def __init__(self, binary, env, home, evidence):
        self.binary, self.env, self.home, self.evidence = binary, dict(env), home, evidence
        self.child = None

    def __enter__(self):
        import secrets
        authority = self.home/'xauthority'
        authority.touch(mode=0o600)
        number = next(number for number in range(19000, 20000)
                      if not Path(f'/tmp/.X11-unix/X{number}').exists() and not Path(f'/tmp/.X{number}-lock').exists())
        self.env.update(DISPLAY=f':{number}', XAUTHORITY=str(authority), GDK_BACKEND='x11')
        self.env.pop('WAYLAND_DISPLAY', None)
        subprocess.run(['/usr/bin/xauth','-f',str(authority),'add',f':{number}', 'MIT-MAGIC-COOKIE-1',secrets.token_hex(16)],
                       env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
        server_env = dict(self.env)
        private_libraries = self.binary.parent.parent/'lib/x86_64-linux-gnu'
        if private_libraries.exists(): server_env['LD_LIBRARY_PATH'] = str(private_libraries)
        self.log = (self.evidence/'xvfb.log').open('w')
        self.child = subprocess.Popen([str(self.binary),f':{number}','-screen','0','1280x900x24','-nolisten','tcp','-auth',str(authority)],
            env=server_env, stdout=self.log, stderr=subprocess.STDOUT, start_new_session=True)
        deadline = time.monotonic()+5
        while self.child.poll() is None and time.monotonic()<deadline:
            try: probe = subprocess.run(['/usr/bin/xwininfo','-root'],env=self.env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=1)
            except subprocess.TimeoutExpired: continue
            if probe.returncode == 0:
                (self.evidence/'display.json').write_text(json.dumps({'isolated':True,'display':f':{number}',
                    'xvfbPid':self.child.pid,'tcpDisabled':True,'waylandDisabled':True},indent=2))
                return self.env
            time.sleep(.05)
        self.__exit__(None,None,None)
        raise RuntimeError('isolated Xvfb did not start; see xvfb.log')

    def __exit__(self, kind, value, traceback):
        if self.child and self.child.poll() is None:
            self.child.terminate()
            try: self.child.wait(timeout=5)
            except subprocess.TimeoutExpired: self.child.kill(); self.child.wait(timeout=5)
        self.log.close()


class IndependentDaemon:
    """An actual daemon/keeper entirely inside this run's private fixture."""
    def __init__(self, binary, env, home, evidence):
        self.binary, self.env, self.home, self.evidence = binary, env, home, evidence
        self.config = Path(env['SPLITTY_CONFIG_DIR'])
        self.identities = []
        self.started = False

    def cli(self, args, input=None):
        output = subprocess.run([str(self.binary), *args], env=self.env, input=input,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20)
        if output.returncode:
            raise RuntimeError(f'fixture daemon command {args}: {output.stderr}')
        return output.stdout

    def keepers(self):
        return sorted((path.name, path.stat().st_ino) for path in (self.config/'run/keepers').glob('*.sock'))

    def __enter__(self):
        try:
            self.started = True
            self.cli(['daemon', 'start'])
            self.job = json.loads(self.cli(['run', '--json', '--', '/bin/sh']))
            self.discovery = (self.config/'daemon.json').read_bytes()
            self.info = json.loads(self.discovery)
            self.daemon = process_identity(self.info['pid'])
            self.identities = [self.daemon]
            self.keeper_pids = []
            for pid in descendants(self.info['pid']):
                try:
                    if b'session-keeper' in Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0'):
                        self.keeper_pids.append(pid)
                        self.identities.append(process_identity(pid))
                except OSError:
                    pass
            self.keeper_sockets = self.keepers()
            if not self.keeper_pids or not self.keeper_sockets:
                raise RuntimeError('fixture job did not acquire an actual keeper')
            self.pid_file = self.home/'independent-job.pid'
            self.cli(['input', self.job['jobId'], '--json'], f"printf '%s\\n' \"$$\" > '{self.pid_file}'\n")
            deadline = time.monotonic()+5
            while not self.pid_file.exists() and time.monotonic()<deadline: time.sleep(.02)
            self.shell = process_identity(int(self.pid_file.read_text()))
            self.identities.append(self.shell)
            return self
        except BaseException:
            self.__exit__(*sys.exc_info())
            raise

    def after_app(self, result):
        marker = self.home/'independent-job-after-app'
        self.cli(['input', self.job['jobId'], '--json'], f"printf '%s\\n' \"$$\" > '{marker}'\n")
        deadline = time.monotonic()+5
        while not marker.exists() and time.monotonic()<deadline: time.sleep(.02)
        same_shell = marker.exists() and int(marker.read_text()) == self.shell['pid']
        status = json.loads(self.cli(['daemon', 'status', '--json']))
        running = json.loads(self.cli(['inspect', self.job['jobId'], '--json']))['state'] == 'running'
        app_instance = result.get('first',{}).get('instance') or result.get('instance')
        check = {'appInstance':app_instance,'daemonInstance': self.info['instance'], 'daemonPid': self.info['pid'],
                 'keeperPids': self.keeper_pids, 'jobShellPid': self.shell['pid'],
                 'discoveryUnchanged': (self.config/'daemon.json').read_bytes() == self.discovery,
                 'keeperSocketsUnchanged': self.keepers() == self.keeper_sockets,
                 'sameDaemonInstance': status['instance'] == self.info['instance'],
                 'daemonAndKeeperAlive': all(identity_alive(saved) for saved in self.identities),
                 'jobRunning': running, 'sameShellAcceptsInputAfterAppExit': same_shell,
                 'appOwnsDifferentInstance': bool(app_instance) and app_instance != self.info['instance']}
        check['success'] = all(check[key] for key in ['discoveryUnchanged','keeperSocketsUnchanged',
            'sameDaemonInstance','daemonAndKeeperAlive','jobRunning','sameShellAcceptsInputAfterAppExit','appOwnsDifferentInstance'])
        (self.evidence/'coexistence.json').write_text(json.dumps(check,indent=2))
        return check

    def __exit__(self, kind, value, traceback):
        if not self.started: return
        try:
            self.cli(['daemon', 'stop', '--end-sessions'])
        finally:
            deadline = time.monotonic()+5
            while any(identity_alive(saved) for saved in self.identities) and time.monotonic()<deadline: time.sleep(.02)
            # Escalate only fixture processes whose exact generation we saved.
            for saved in reversed(self.identities):
                if identity_alive(saved): os.kill(saved['pid'], signal.SIGKILL)
            deadline = time.monotonic()+5
            while any(identity_alive(saved) for saved in self.identities) and time.monotonic()<deadline: time.sleep(.02)
            clean = not any(identity_alive(saved) for saved in self.identities)
            (self.evidence/'daemon-cleanup.json').write_text(json.dumps({'success':clean,'processes':self.identities},indent=2))
            if not clean: raise RuntimeError('fixture daemon/keeper cleanup failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence-dir', type=Path)
    parser.add_argument('--prepare-only', action='store_true', help='generate the temporary harness without building or running GUI')
    parser.add_argument('--xvfb', type=Path, default=shutil.which('Xvfb'), help='Xvfb executable; all GUI runs use an isolated display')
    parser.add_argument('--reloads', type=int, default=1, help='actual document reloads (1..64); full UI checks heartbeats in first and last documents')
    parser.add_argument('--coexist-daemon', action='store_true', help='prove app lifetime is independent from a real fixture daemon/keeper job')
    parser.add_argument('--isolation-consumer', action='store_true', help='actual Main/two-Remote policies and bounded receipt-starved client; fixed fixture backends, no SSH')
    parser.add_argument('--full-ui', action='store_true', help='package frontend/dist and exercise the production Svelte application/assets')
    parser.add_argument('--harness-dir', type=Path, default=Path('/tmp/splitty-webkit-runtime-smoke'))
    args = parser.parse_args()
    if not 1 <= args.reloads <= 64: parser.error('--reloads must be between 1 and 64')
    if args.isolation_consumer and args.full_ui: parser.error('--isolation-consumer is a separate mode; omit --full-ui')
    repo = Path(__file__).resolve().parents[2]
    harness = args.harness_dir.resolve()
    if sys.platform != 'linux':
        raise RuntimeError('this actual WebKitGTK smoke requires Linux')
    if not args.prepare_only and (not args.xvfb or not args.xvfb.is_file()):
        raise RuntimeError('an isolated Xvfb executable is required (--xvfb); the user desktop is never used')
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
name = "splitty-webkit-runtime-smoke"
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
webkit2gtk = { version = "2", features = ["v2_40"] }
gtk = "0.18"
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
    source += '\n' + (repo/'scripts/testing/tauri-webkit-smoke/report.rs').read_text()
    if args.isolation_consumer: source = _isolation.instrument(harness,repo,source)
    if args.isolation_consumer: shutil.copyfile(harness/'backend-substitution.json',evidence/'backend-substitution.json')
    source += '\nfn main() { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1"); run(); }\n'
    (harness/'src/main.rs').write_text(source)
    source_files = ['crates/splitty-tauri/Cargo.toml', 'crates/splitty-tauri/tauri.conf.json',
                    'crates/splitty-tauri/build.rs', 'crates/splitty-tauri/src/lib.rs', 'crates/splitty-tauri/src/navigation.rs',
                    'crates/splitty-tauri/src/client_ipc.rs', 'crates/splitty-tauri/src/client_ipc/bootstrap.js',
                    'crates/splitty-tauri/src/client_ipc/carrier.rs', 'crates/splitty-tauri/src/client_ipc/provider.rs',
                    'frontend/src/lib/protocol/native_socket.ts', 'frontend/src/lib/protocol/tauri_channel.ts']
    source_files += [str(path.relative_to(repo)) for path in (repo/'scripts/testing/tauri-webkit-smoke').rglob('*') if path.is_file() and '__pycache__' not in path.parts]
    source_files += ['scripts/testing/tauri-windows-native-smoke/pressure.mjs','frontend/src/lib/terminal/session_client.ts','frontend/src/lib/terminal/attach_registry.ts']
    (evidence/'sources.json').write_text(json.dumps({name:hashlib.sha256((repo/name).read_bytes()).hexdigest() for name in source_files}, indent=2))
    vite = repo/'frontend/node_modules/vite/dist/node/index.js'
    config = f'''import {{ defineConfig }} from {json.dumps(str(vite))};
export default defineConfig({{ build: {{ outDir: {json.dumps(str(harness/'src'))}, emptyOutDir:false, minify:false,
lib: {{ entry: {json.dumps(str(repo/'scripts/testing/tauri-webkit-smoke/entry.ts'))}, name:'SplittyRuntimeSmoke', formats:['iife'], fileName:()=> 'smoke.js' }}, target:'es2022' }}, define: {{ __SPLITTY_FULL_UI__: {str(args.full_ui).lower()}, __SPLITTY_SMOKE_RELOADS__: {args.reloads}, __SPLITTY_ISOLATION_CONSUMER__: {str(args.isolation_consumer).lower()} }} }});
'''
    (harness/'vite.config.mjs').write_text(config)
    env = dict(os.environ, CARGO_TARGET_DIR=str(repo/'target'))
    if args.prepare_only:
        (evidence/'preparation.json').write_text(json.dumps({'runtimeVerified':False,'harness':str(harness),'isolationConsumer':args.isolation_consumer,'backendSubstitution':'two fixed real embedded services; no SSH'}))
        print(f'Prepared {harness}; actual WebKit runtime UNVERIFIED')
        return
    for command, logfile in [([str(repo/'frontend/node_modules/.bin/vite'),'build','--config',str(harness/'vite.config.mjs')], 'bundle.log'),
                             (['cargo','build','--manifest-path',str(harness/'Cargo.toml')], 'build.log')]:
        with (evidence/logfile).open('w') as log:
            subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
    with tempfile.TemporaryDirectory(prefix='splitty-webkit-home-') as home:
        home = Path(home)
        env = {key:value for key,value in env.items() if not key.startswith('SPLITTY_')}
        env.pop('ZDOTDIR', None)
        env.update(HOME=str(home), XDG_CONFIG_HOME=str(home/'config'), XDG_DATA_HOME=str(home/'data'),
                   XDG_CACHE_HOME=str(home/'cache'), SPLITTY_SMOKE_EVIDENCE=str(evidence), WEBKIT_DISABLE_DMABUF_RENDERER='1')
        # WSLg Wayland's socket belongs to its existing runtime directory. Keep
        # that display carrier; isolate the server control path separately.
        env['SPLITTY_CONFIG_DIR'] = str(home/'splitty')
        with contextlib.ExitStack() as cleanup:
            env = cleanup.enter_context(IsolatedDisplay(args.xvfb.resolve(), env, home, evidence))
            daemon = cleanup.enter_context(IndependentDaemon(repo/'target/debug/splitty', env, home, evidence)) if args.coexist_daemon else None
            with (evidence/'runtime.log').open('w') as log:
                child = subprocess.Popen([str(repo/'target/debug/splitty-webkit-runtime-smoke')], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                samples = []
                observed_processes = set()
                shell_processes = {}
                listener_samples = 0
                try:
                    timeout = 180+6*args.reloads if args.isolation_consumer else (90+6*args.reloads if args.full_ui else 45+3*args.reloads)
                    deadline = time.monotonic()+timeout
                    while child.poll() is None and time.monotonic()<deadline:
                        samples += listeners(child.pid)
                        listener_samples += 1
                        for pid in descendants(child.pid):
                            try:
                                name = Path(f'/proc/{pid}/comm').read_text().strip()
                                observed_processes.add(name)
                                if name in ['bash', 'zsh', 'sh', 'dash', 'fish']:
                                    shell_processes[pid] = Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19]
                            except (OSError, IndexError): pass
                        time.sleep(.05)
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGTERM)
                        child.wait(timeout=5)
                        raise RuntimeError(f'actual WebKit smoke timed out; see {evidence}')
                    result_path = evidence/'result.json'
                    result = json.loads(result_path.read_text()) if result_path.exists() else {'error':'app exited without report', 'exitCode':child.returncode}
                    terminal_pids = [value.get('terminalPid') for value in [result.get('first', {}), result.get('reloaded', {})]]
                    terminal_pids_exited = all(isinstance(pid, int) and not Path(f'/proc/{pid}').exists() for pid in terminal_pids)
                    def shell_alive(pid, birth):
                        try: return Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19] == birth
                        except (OSError, IndexError): return False
                    cleanup_deadline = time.monotonic()+5
                    while any(shell_alive(pid,birth) for pid,birth in shell_processes.items()) and time.monotonic()<cleanup_deadline:
                        time.sleep(.05)
                    result['ownedShellPids'] = sorted(shell_processes)
                    result['ownedShellPidsExited'] = not any(shell_alive(pid,birth) for pid,birth in shell_processes.items())
                    result['terminalPidsExited'] = terminal_pids_exited
                    result['mode'] = 'isolation-consumer' if args.isolation_consumer else 'full-ui' if args.full_ui else 'minimal'
                    result['packagedAssets'] = {str(path.relative_to(harness/'assets')):hashlib.sha256(path.read_bytes()).hexdigest() for path in (harness/'assets').rglob('*') if path.is_file()}
                    result['tcpListeners'] = samples
                    result['listenerSamples'] = listener_samples
                    result['observedProcesses'] = sorted(observed_processes)
                    result['appExitCode'] = child.returncode
                    if daemon: result['coexistence'] = daemon.after_app(result)
                    result_path.write_text(json.dumps(result, indent=2))
                    print(json.dumps(result, indent=2))
                    if child.returncode != 0 or not result.get('success') or not terminal_pids_exited or not result['ownedShellPidsExited'] or samples or (daemon and not result['coexistence']['success']):
                        raise RuntimeError(f'actual WebKit smoke failed; see {evidence}')
                finally:
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGKILL)
                        child.wait()


if __name__ == '__main__':
    main()
