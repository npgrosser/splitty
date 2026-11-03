#!/usr/bin/env python3
"""Run installed native Windows MSVC/WebView2 checks from a WSL checkout.

Copies only source, fixtures and already-built frontend assets into a new Windows
TEMP directory. No application/tool installation or WSL provisioning occurs.
The optional runtime probe uses production Tauri bootstrap/broker code with a
unique test identifier, private WebView2 data directory and packaged setup-role
fixture. It proves setup IPC/role denial without claiming a live WSL/SSH backend.
"""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import uuid

HERE=Path(__file__).resolve().parent

PS = '/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe'

def powershell(source, log=None):
    source = "$ProgressPreference='SilentlyContinue'\n" + source
    encoded = base64.b64encode(source.encode('utf-16le')).decode()
    command = [PS, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-EncodedCommand', encoded]
    if log:
        with log.open('w') as output:
            result = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT)
            if result.returncode: raise RuntimeError(f'Windows step failed ({result.returncode}); see {log}')
    else:
        return subprocess.check_output(command, text=True).strip()

def quote(value):
    return "'" + str(value).replace("'", "''") + "'"

def windows(path):
    return subprocess.check_output(['wslpath', '-w', str(path)], text=True).strip()

def hashes(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for base in ['crates', 'docs', 'testdata', 'frontend/dist', 'architecture']
            for path in sorted((root/base).rglob('*')) if path.is_file() and
            'target' not in path.parts and 'gen' not in path.parts}

def copy_workspace(repo, payload):
    temp = powershell('$env:TEMP')
    root = Path(subprocess.check_output(['wslpath', '-u', temp], text=True).strip()) / ('splitty-native-windows-' + uuid.uuid4().hex[:12])
    root.mkdir()
    for name in ['crates', 'docs', 'testdata', 'frontend/dist', 'architecture']:
        shutil.copytree(repo/name, root/name, ignore=shutil.ignore_patterns('node_modules', 'target', 'gen', '.likec4', 'build'))
    for name in ['Cargo.toml', 'Cargo.lock', 'CLAUDE.md']:
        shutil.copyfile(repo/name, root/name)
    (root/'source-hashes.json').write_text(json.dumps(hashes(root), indent=2))
    resource = root/'crates/splitty-tauri/resources'
    archive = resource/'splitty-wsl-x86_64.tar.gz'
    with tarfile.open(archive, 'w:gz', compresslevel=1) as output:
        output.add(payload, arcname='splitty')
    (resource/'splitty-wsl-x86_64.tar.gz.sha256').write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+'\n')
    (root/'TEST-ONLY.txt').write_text('Temporary Windows validation copy. WSL archive contains a test-only Linux debug executable, not a release payload. Never provision an existing distribution.\n')
    return root

def cargo(root, arguments, name):
    source = f'''$ErrorActionPreference='Continue'
$env:PATH="$env:USERPROFILE\\.cargo\\bin;$env:PATH"
$env:CARGO_TARGET_DIR={quote(windows(root/'target'))}
Set-Location {quote(windows(root))}
& "$env:USERPROFILE\\.cargo\\bin\\cargo.exe" {arguments}
exit $LASTEXITCODE
'''
    powershell(source, root/name)

def instrument(root):
    crate = root/'crates/splitty-tauri'
    source = (crate/'src/lib.rs').read_text()
    if 'windows_smoke_report' in source:
        raise RuntimeError('runtime copy already instrumented; create a fresh snapshot')
    (root/'production-lib.rs').write_text(source)
    source = source.replace('            native_bootstrap_begin,', '            windows_smoke_report,\n            native_bootstrap_begin,', 1)
    source = source.replace('            app.add_capability(capability::main_window())?;', '''            app.add_capability(capability::main_window())?;
            app.add_capability(tauri::ipc::CapabilityBuilder::new("windows-smoke-report").window("main").local(true).permission("allow-windows-smoke-report"))?;''', 1)
    source = source.replace('.background_color(Color(18, 18, 20, 255));', '.background_color(Color(18, 18, 20, 255)).data_directory(std::path::PathBuf::from(std::env::var_os("SPLITTY_SMOKE_WEBVIEW_DATA").expect("fixture WebView data"))).visible(false);', 1)
    source += '''
#[cfg(target_os = "windows")]
#[tauri::command]
fn windows_smoke_report(app: AppHandle, webview: tauri::Webview, native: tauri::State<'_, Arc<Native>>, mut value: serde_json::Value) -> Result<(), String> {
    value["actualUrl"] = serde_json::json!(webview.url().map_err(|error| error.to_string())?.as_str());
    value["nativeDocumentCommitted"] = serde_json::json!(native.broker.current_document(webview.label()).is_some());
    value["wslOwnerStarted"] = serde_json::json!(app.state::<Arc<windows_wsl::platform::WindowsWslState>>().owner().is_ok());
    let failed = value.get("error").is_some() || value["wslOwnerStarted"] == true || value["nativeDocumentCommitted"] != true;
    std::fs::write(std::path::PathBuf::from(std::env::var_os("SPLITTY_SMOKE_EVIDENCE").expect("fixture evidence")).join("runtime-result.json"), serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    app.exit(if failed { 1 } else { 0 });
    Ok(())
}
'''
    (crate/'src/lib.rs').write_text(source)
    build = (crate/'build.rs').read_text().replace('            "native_bootstrap_begin",', '            "windows_smoke_report",\n            "native_bootstrap_begin",', 1)
    (crate/'build.rs').write_text(build)
    config = json.loads((crate/'tauri.conf.json').read_text())
    config['identifier'] = 'com.splitty.windows-smoke-' + uuid.uuid4().hex[:12]
    config['productName'] = 'Splitty Windows Native Smoke'
    config['build']['frontendDist'] = 'runtime-assets'
    config['bundle']['active'] = False
    (crate/'tauri.conf.json').write_text(json.dumps(config, indent=2))
    assets = crate/'runtime-assets'; assets.mkdir()
    (assets/'setup.html').write_text('<!doctype html><html><head><title>Splitty Windows smoke</title><script type="module" src="/probe.js"></script></head><body>Isolated setup-role native IPC probe</body></html>')
    (assets/'probe.js').write_text('''
const report = value => window.__TAURI_INTERNALS__.invoke('windows_smoke_report', {value});
const assert = (condition, message) => { if (!condition) throw Error(message); };
const timer = setTimeout(() => { void report({error:'native setup probe timeout'}); }, 25000);
void (async () => {
  const capsule = window.__SPLITTY_TAURI__;
  assert(capsule, 'native capsule missing');
  const metadata = await capsule.ready;
  assert(metadata.role === 'setup', 'native setup role missing');
  assert(!('auth' in capsule) && !('secret' in metadata), 'private proof exposed');
  let deniedOpen = false, deniedEditor = false, invalidTarget = false;
  try { await capsule.native.open(undefined, 7); } catch { deniedOpen = true; }
  try { await capsule.invoke('open_local_editor', {cwd:'/tmp',editor:'vscode'}); } catch { deniedEditor = true; }
  try { await capsule.invoke('remote_open', {target:'-oProxyCommand=invalid'}); } catch { invalidTarget = true; }
  assert(deniedOpen && deniedEditor && invalidTarget, 'setup command scope failed');
  await new Promise(resolve => setTimeout(resolve, 5000));
  clearTimeout(timer);
  await report({role:metadata.role,deniedOpen,deniedEditor,invalidTarget,userAgent:navigator.userAgent});
})().catch(error => { clearTimeout(timer); void report({error:String(error)}); });
''')
    (root/'runtime-source-hashes.json').write_text(json.dumps(hashes(root), indent=2))

def runtime(root, allow_wsl=False, max_seconds=35):
    raise RuntimeError("GUI disabled pending independent STARTUPINFOEX stdio review")
    evidence = root/'runtime-evidence'; evidence.mkdir(exist_ok=True)
    for name in ['runtime-result.json', 'processes.json', 'stdout.log', 'stderr.log']: (evidence/name).unlink(missing_ok=True)
    shutil.copyfile(HERE/'tauri-windows-native-smoke/fixture-job.cs',root/'fixture-job.cs')
    source = f'''$ErrorActionPreference='Stop'
$env:SPLITTY_SMOKE_EVIDENCE={quote(windows(evidence))}
$env:SPLITTY_SMOKE_WEBVIEW_DATA={quote(windows(evidence/'webview-data'))}
Add-Type -Path {quote(windows(root/'fixture-job.cs'))}
$job=$null; $listeners=@(); $remaining=@(); $members=@(); $timedOut=$false; $exitCode=$null; $zero=$false; $launch=$null; $stdoutPath=$null; $stderrPath=$null
try {{
  $job=[SplittyFixtureJob]::new({quote(windows(root/'target/debug/splitty-gui.exe'))},[string[]]@(),{quote(windows(root))},$false)
  $launch=$job.LaunchedCommandLine; $stdoutPath=$job.StandardOutputPath; $stderrPath=$job.StandardErrorPath
  $deadline=(Get-Date).AddSeconds({max_seconds})
  do {{
    $members=@($job.Snapshot()); $ids=@($members | ForEach-Object {{$_.Id}})
    if($ids.Count){{$listeners += @(Get-NetTCPConnection -State Listen -OwningProcess $ids -ErrorAction SilentlyContinue | Select-Object LocalAddress,LocalPort,OwningProcess)}}
    if($job.ActiveProcesses -eq 0){{break}}
    Start-Sleep -Milliseconds 100
  }} while((Get-Date) -lt $deadline)
  $timedOut=$job.ActiveProcesses -ne 0; $exitCode=$job.RootExitCode; $remaining=@($job.Snapshot())
}} finally {{
  if($job){{try{{$zero=$job.Stop(5000);$members=@($job.History())}}finally{{$job.Dispose()}}}}
  $result=[ordered]@{{ExitCode=$exitCode;TimedOut=$timedOut;LaunchedCommandLine=$launch;StdOutPath=$stdoutPath;StdErrPath=$stderrPath;Processes=$members;Listeners=$listeners;WslWorkers=@($members|Where-Object{{[System.IO.Path]::GetFileName($_.ExecutablePath)-match '^wsl([.]exe|host[.]exe)$'}});RemainingOwned=$remaining;JobEmptyAfterCleanup=$zero;OwnedHandlesAfterCleanup=[SplittyFixtureJob]::LiveHandleCount}}
  $result|ConvertTo-Json -Depth 5|Out-File -Encoding utf8 {quote(windows(evidence/'processes.json'))}
}}
if($timedOut -or $exitCode -ne 0 -or $listeners.Count -or -not $zero -or [SplittyFixtureJob]::LiveHandleCount -ne 0 -or (($result.WslWorkers.Count -gt 0) -ne ${str(allow_wsl).lower()})){{exit 1}}
'''
    powershell(source,evidence/'driver.log')
    result=json.loads((evidence/'runtime-result.json').read_text())
    if result.get('error') or result.get('wslOwnerStarted') != allow_wsl or not result.get('nativeDocumentCommitted'):
        raise RuntimeError('native WebView2 probe failed: '+json.dumps(result))
    return evidence

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', type=Path, help='reuse a previously created Windows TEMP snapshot')
    parser.add_argument('--linux-test-payload', type=Path, help='test-only Linux executable for mandatory WSL bundle resources')
    parser.add_argument('--skip-tests', action='store_true', help='run only the WebView2 probe on an already-tested snapshot')
    parser.add_argument('--reuse-runtime', action='store_true', help='reuse already instrumented temporary fixture after updating copied sources')
    parser.add_argument('--build-only', action='store_true', help='native MSVC GUI unit tests only')
    args=parser.parse_args(); repo=Path(__file__).resolve().parents[2]
    root=args.workspace.resolve() if args.workspace else copy_workspace(repo, args.linux_test_payload or repo/'target/debug/splitty')
    if not (root/'TEST-ONLY.txt').is_file() or not root.name.startswith('splitty-native-windows-'):
        raise RuntimeError('refusing to modify anything except an explicitly marked temporary fixture')
    print(root, flush=True)
    if not args.skip_tests: cargo(root, 'test --locked -p splitty-gui --lib', 'windows-gui-test.log')
    if not args.build_only:
        if not args.reuse_runtime: instrument(root)
        (root/'runtime-source-hashes.json').write_text(json.dumps(hashes(root), indent=2))
        cargo(root, 'build --locked -p splitty-gui --bin splitty-gui', 'windows-runtime-build.log')
        print(runtime(root), flush=True)

if __name__=='__main__': main()
