#!/usr/bin/env python3
"""Hidden WebView2 + two disposable WSL native-backend fixtures (WSL host).

Never targets an existing distribution or changes defaults/.wslconfig. See the
fixture README for isolation shims, cleanup guards and exact coverage limits.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path, PureWindowsPath
import re
import shutil
import subprocess
import tarfile
import uuid

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('native_windows_smoke', HERE/'tauri-windows-native-smoke.py')
common=importlib.util.module_from_spec(spec); spec.loader.exec_module(common)

def registrations():
    value=common.powershell('ConvertTo-Json -InputObject @(Get-ChildItem HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Lxss | ForEach-Object { Get-ItemProperty $_.PSPath | Select-Object DistributionName,BasePath }) -Compress')
    return {row['DistributionName']:row['BasePath'] for row in json.loads(value)}

WSL_EXECUTION_APPROVED=False
OWNED_TARGETS={}
OWNED_ROOTS={}
GUEST_GUARD=(HERE/'tauri-windows-native-smoke/fixture-guest-guard.sh').read_text().strip()

def guarded_guest_arguments(name,arguments):
    if not re.fullmatch(r'splitty-native-fixture-[0-9a-f]{12}-[ab]',name):raise RuntimeError('not a private fixture name')
    return ['--distribution',name,'--exec','/bin/sh','-c',GUEST_GUARD,'splitty-fixture-guard',name,*arguments]

def wsl(name, arguments, log=None, user=None):
    if not WSL_EXECUTION_APPROVED:raise RuntimeError('explicit --disposable-distros scoped approval required')
    if name not in OWNED_TARGETS:raise RuntimeError('unowned fixture target')
    found=registrations().get(name,'').removeprefix('\\\\?\\').rstrip('\\/').casefold()
    if found!=OWNED_TARGETS[name].rstrip('\\/').casefold():raise RuntimeError('fixture registration changed')
    if user not in [None,'root']:raise RuntimeError('unsupported fixture user')
    root=OWNED_ROOTS[name]
    source='$ErrorActionPreference="Stop"\n$RootDir='+common.quote(common.windows(root))+'\n$ownership=Get-Content (Join-Path $RootDir "owned-distros.json") -Raw|ConvertFrom-Json\nAdd-Type -Path (Join-Path $RootDir "fixture-job.cs")\n. (Join-Path $RootDir "fixture-sentinel.ps1")\n$guestArgs=[string[]](ConvertFrom-Json '+common.quote(json.dumps(arguments))+')\nInvoke-FixtureGuest -Name '+common.quote(name)+' -Arguments $guestArgs'+(' -User '+common.quote(user) if user else '')
    return common.powershell(source,log)

def rootfs(path):
    files={}
    tools=['sh','bash','id','uname','cat','printenv','mkdir','chmod','mv','rm','ln','tar','gzip','tee','test','sleep','ps','kill','sha256sum','getent','readlink']
    for name in tools:
        source=Path(shutil.which(name) or '/bin/'+name)
        if not source.is_file(): raise RuntimeError('missing local fixture tool '+name)
        files['usr/bin/'+name]=source.resolve()
        result=subprocess.check_output(['ldd',str(source.resolve())],text=True)
        for line in result.splitlines():
            match=re.search(r'=> (/\S+)',line) or re.search(r'^\s*(/\S+)',line)
            if match:
                library=Path(match.group(1)); files[str(library).lstrip('/')]=library.resolve()
                if str(library).startswith('/usr/lib/'): files[str(library)[5:]]=library.resolve()
    # Loader/lib dependencies of the actual helper, not only shell tools.
    binary=Path.cwd()/'target/debug/splitty'
    for line in subprocess.check_output(['ldd',str(binary)],text=True).splitlines():
        match=re.search(r'=> (/\S+)',line) or re.search(r'^\s*(/\S+)',line)
        if match:
            library=Path(match.group(1)); files[str(library).lstrip('/')]=library.resolve()
            if str(library).startswith('/usr/lib/'): files[str(library)[5:]]=library.resolve()
    text={
      'etc/passwd':'root:x:0:0:root:/root:/bin/sh\nsplitty-fixture:x:1000:1000:fixture:/home/splitty-fixture:/bin/sh\n',
      'etc/group':'root:x:0:\nsplitty-fixture:x:1000:\n',
      'etc/nsswitch.conf':'passwd: files\ngroup: files\nhosts: files dns\n',
      'etc/shells':'/bin/sh\n/bin/bash\n',
      'etc/os-release':'ID=ubuntu\nVERSION_ID="24.04"\nNAME="Splitty isolated Ubuntu-compatible fixture"\n',
      'etc/fstab':'',
      'etc/wsl.conf':'[boot]\nsystemd=false\n[automount]\nenabled=false\nmountFsTab=false\n[interop]\nenabled=false\nappendWindowsPath=false\n[user]\ndefault=splitty-fixture\n',
      'etc/hosts':'127.0.0.1 localhost\n::1 localhost\n',
    }
    with tarfile.open(path,'w') as archive:
        for name in ['usr','usr/bin','etc','lib','lib64','usr/lib','tmp','run','root','home','home/splitty-fixture','dev','proc','sys']:
            info=tarfile.TarInfo(name); info.type=tarfile.DIRTYPE; info.mode=0o1777 if name=='tmp' else 0o755
            if name=='home/splitty-fixture': info.uid=info.gid=1000; info.mode=0o700
            archive.addfile(info)
        info=tarfile.TarInfo('bin'); info.type=tarfile.SYMTYPE; info.linkname='usr/bin'; archive.addfile(info)
        linked={}
        for name,source in files.items():
            identity=(source.stat().st_dev,source.stat().st_ino)
            info=tarfile.TarInfo(name); info.mode=0o755
            if identity in linked:
                info.type=tarfile.LNKTYPE; info.linkname=linked[identity]; archive.addfile(info)
            else:
                linked[identity]=name; info.size=source.stat().st_size
                with source.open('rb') as data: archive.addfile(info,data)
        for name,value in text.items():
            data=value.encode(); info=tarfile.TarInfo(name); info.size=len(data); info.mode=0o644; archive.addfile(info,io.BytesIO(data))

@contextlib.contextmanager
def fixtures(root,count=2):
    if not WSL_EXECUTION_APPROVED:raise RuntimeError('explicit --disposable-distros scoped approval required')
    if count not in [1,2]:raise RuntimeError('fixture count must be one or two')
    for filename in ['fixture-job.cs','fixture-sentinel.ps1','fixture-guest-guard.sh']:
        shutil.copyfile(HERE/'tauri-windows-native-smoke'/filename,root/filename)
    names=['splitty-native-fixture-'+uuid.uuid4().hex[:12]+suffix for suffix in ['-a','-b'][:count]]
    before=registrations(); owned={}; archive=root/'fixture-rootfs.tar'; rootfs(archive)
    (root/'owned-distros.json').write_text(json.dumps({'before':before,'created':owned,'names':names},indent=2))
    try:
        for name in names:
            if name in registrations() or name in before: raise RuntimeError('fixture name already registered')
            location=root/'distributions'/name; location.mkdir(parents=True)
            owned[name]=common.windows(location)
            OWNED_TARGETS[name]=owned[name]
            OWNED_ROOTS[name]=root
            # Persist ownership before invoking import; finally also handles a partial import.
            (root/'owned-distros.json').write_text(json.dumps({'before':before,'created':owned,'names':names},indent=2))
            common.powershell('& wsl.exe --import '+common.quote(name)+' '+common.quote(common.windows(location))+' '+common.quote(common.windows(archive))+' --version 2\nexit $LASTEXITCODE',root/(name+'-import.log'))
            actual=wsl(name,['/usr/bin/id','-u']).strip()
            if actual!='1000': raise RuntimeError('fixture default user is not non-root: '+repr(actual))
            home=wsl(name,['/usr/bin/printenv','HOME']).strip()
            if home!='/home/splitty-fixture': raise RuntimeError('fixture HOME not isolated: '+repr(home))
        yield names
    finally:
        cleanup=[]
        for name,location in owned.items():
            found=registrations()
            if name not in found: continue
            actual=found[name].removeprefix('\\\\?\\').rstrip('\\/').casefold()
            if name in before or actual!=location.rstrip('\\/').casefold():
                cleanup.append({'name':name,'error':'ownership guard refused cleanup'}); continue
            try:
                common.powershell('& wsl.exe --terminate '+common.quote(name),root/(name+'-terminate.log'))
                common.powershell('& wsl.exe --unregister '+common.quote(name)+'\nexit $LASTEXITCODE',root/(name+'-unregister.log'))
                cleanup.append({'name':name,'unregistered':name not in registrations()})
            except Exception as error: cleanup.append({'name':name,'error':str(error)})
        for name in owned:
            OWNED_TARGETS.pop(name,None);OWNED_ROOTS.pop(name,None)
        (root/'distro-cleanup.json').write_text(json.dumps(cleanup,indent=2))
        if any(row.get('error') or not row.get('unregistered') for row in cleanup): raise RuntimeError('fixture cleanup needs inspection: '+str(root/'distro-cleanup.json'))

def instrument(root,names,repo,full_ui=False):
    common.instrument(root)
    crate=root/'crates/splitty-tauri'; path=crate/'src/windows_wsl.rs'; source=path.read_text()
    source=source.replace('fn config_path(app: &AppHandle) -> Result<PathBuf, String> {','fn config_path(app: &AppHandle) -> Result<PathBuf, String> {\n        if let Some(path) = std::env::var_os("SPLITTY_SMOKE_APP_DATA") { return Ok(PathBuf::from(path).join("windows.json")); }',1)
    anchor='''            self.check_cancelled()?;
            let mut command = wsl_command();'''
    shim='''            self.check_cancelled()?;
            let names_data = std::env::var_os("SPLITTY_SMOKE_DISTROS").ok_or("fixture target list absent")?;
            let names: Vec<String> = serde_json::from_str(&names_data.to_string_lossy()).map_err(|e| e.to_string())?;
            if arguments == [OsString::from("--list"), OsString::from("--verbose")] {
                return Ok(ProcessOutput { success:true, stdout:names.iter().map(|name| format!("  {name} Running 2\\n")).collect::<String>().into_bytes(), stderr:Vec::new() });
            }
            if arguments.len() < 4 || arguments[0] != "-d" || arguments[2] != "--exec" || !names.iter().any(|name| arguments[1] == name.as_str()) { return Err("fixture guard rejected an unowned WSL target".into()); }
            let mut fixture_arguments = arguments[..3].to_vec();
            fixture_arguments[0] = OsString::from("--distribution");
            fixture_arguments.extend([OsString::from("/bin/sh"), OsString::from("-c"), OsString::from(GUEST_GUARD_LITERAL), OsString::from("splitty-fixture-guard"), arguments[1].clone()]);
            fixture_arguments.extend_from_slice(&arguments[3..]);
            let arguments = &fixture_arguments;
            let mut command = wsl_command();'''.replace('GUEST_GUARD_LITERAL',json.dumps(GUEST_GUARD))
    if source.count(anchor)!=1: raise RuntimeError('WSL executor instrumentation anchor changed')
    source=source.replace(anchor,shim,1)
    owner_start=source.index("fn native_backend_arguments<'a>");owner_end=source.index('\n}\n',owner_start)+2
    owner_guard='''fn native_backend_arguments(distro: &str, binary: &str) -> Vec<String> {
    let names: Vec<String> = serde_json::from_str(&std::env::var("SPLITTY_SMOKE_DISTROS").expect("fixture target list absent")).expect("fixture target list malformed");
    assert!(names.iter().any(|name| name == distro), "unowned fixture backend target");
    vec!["--distribution".into(), distro.into(), "--exec".into(), "/bin/sh".into(), "-c".into(), GUEST_GUARD_LITERAL.into(), "splitty-fixture-guard".into(), distro.into(), binary.into(), "desktop-backend".into(), "--transport".into(), "native-v1".into(), "--backend".into(), "embedded".into(), "--owner".into()]
}'''.replace('GUEST_GUARD_LITERAL',json.dumps(GUEST_GUARD))
    source=source[:owner_start]+owner_guard+source[owner_end:];path.write_text(source)
    path=crate/'src/lib.rs'; source=path.read_text(); start=source.rindex('#[cfg(target_os = "windows")]\n#[tauri::command]\nfn windows_smoke_report')
    source=source[:start]+'''
#[cfg(target_os = "windows")]
#[tauri::command]
fn windows_smoke_report(app: AppHandle, webview: tauri::Webview, native: tauri::State<'_, Arc<Native>>, mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    let owner = app.state::<Arc<windows_wsl::platform::WindowsWslState>>().owner();
    value["actualUrl"] = serde_json::json!(webview.url().map_err(|error| error.to_string())?.as_str());
    value["nativeDocumentCommitted"] = serde_json::json!(native.broker.current_document(webview.label()).is_some());
    value["wslOwnerStarted"] = serde_json::json!(owner.is_ok());
    value["wslOwnerPid"] = serde_json::json!(owner.as_ref().ok().and_then(|owner| owner.id()));
    let phase = value["phase"].as_str().unwrap_or("error");
    let final_report = phase == "final" || value.get("error").is_some();
    let filename = if final_report { "runtime-result.json".to_owned() } else if ["started-a","duplicate-a","started-b","main-a","native-open"].contains(&phase) { format!("checkpoint-{phase}.json") } else { return Err("invalid fixture checkpoint".into()); };
    std::fs::write(std::path::PathBuf::from(std::env::var_os("SPLITTY_SMOKE_EVIDENCE").expect("fixture evidence")).join(filename), serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if final_report { app.exit(if value.get("error").is_some() { 1 } else { 0 }); }
    Ok(value)
}
'''
    path.write_text(source)
    assets=crate/'runtime-assets'
    for name in ['setup.html','index.html']:
        (assets/name).write_text('<!doctype html><html><head><title>Isolated Windows WSL native smoke</title><script type="module" src="/probe.js"></script></head><body>Hidden native pipe fixture</body></html>')
    if full_ui:
        prepare_full_ui(crate,assets,repo,root)
    config=root/'wsl-vite.config.mjs'
    config.write_text('import {defineConfig} from '+json.dumps(str(repo/'frontend/node_modules/vite/dist/node/index.js'))+'; export default defineConfig({build:{outDir:'+json.dumps(str(assets))+',emptyOutDir:false,minify:false,lib:{entry:'+json.dumps(str(HERE/'tauri-windows-native-smoke/wsl-entry.ts'))+',name:"WslNativeProbe",formats:["iife"],fileName:()=>"probe.js"},target:"es2022"},define:{__FIXTURES__:'+json.dumps(json.dumps(names))+',__WINDOWS_FULL_UI__:'+json.dumps(full_ui)+'}});')
    with (root/'wsl-probe-bundle.log').open('w') as output:
        subprocess.run([str(repo/'frontend/node_modules/.bin/vite'),'build','--config',str(config)],cwd=repo,stdout=output,stderr=subprocess.STDOUT,check=True)
    (root/'runtime-source-hashes.json').write_text(json.dumps(common.hashes(root),indent=2))

def prepare_full_ui(crate,assets,repo,root):
    dist=repo/'frontend/dist'
    if not (dist/'index.html').is_file() or not (dist/'popout.html').is_file():raise RuntimeError('build frontend/dist before --full-ui')
    source_assets={str(path.relative_to(dist)):hashlib.sha256(path.read_bytes()).hexdigest() for path in dist.rglob('*') if path.is_file()}
    setup=(assets/'setup.html').read_text()
    shutil.copytree(dist,assets,dirs_exist_ok=True)
    (assets/'setup.html').write_text(setup) # setup remains the existing isolated backend controller
    index=(assets/'index.html').read_text()
    if '</body>' not in index:raise RuntimeError('production index body missing')
    (assets/'index.html').write_text(index.replace('</body>','<script type="module" src="/probe.js"></script></body>',1))
    (root/'production-ui-assets.json').write_text(json.dumps({'assets':source_assets,'mode':'real production index plus separate probe; setup controller unchanged'},indent=2))
    helpers=(HERE/'tauri-windows-native-smoke/document.mjs').read_text().replace('export function','function')
    observer=(HERE/'tauri-windows-native-smoke/full-ui-init.js').read_text()
    (crate/'src/windows-full-ui-init.js').write_text('(function(){\n'+helpers+'\n'+observer+'\n})();')
    path=crate/'src/lib.rs';source=path.read_text()
    source=source.replace('            windows_smoke_report,','            windows_smoke_report,\n            smoke_windows,',1)
    source=source.replace('.permission("allow-windows-smoke-report")','.permission("allow-windows-smoke-report").permission("allow-smoke-windows")',1)
    anchor='.initialization_script(script)'
    if source.count(anchor)!=1:raise RuntimeError('full UI initialization anchor changed')
    source=source.replace(anchor,anchor+'.initialization_script(include_str!("windows-full-ui-init.js"))',1)
    source+='''
#[cfg(target_os = "windows")]
#[tauri::command]
fn smoke_windows(app: AppHandle) -> Result<serde_json::Value, String> {
    let windows=app.webview_windows();
    let mut managed: Vec<_>=windows.keys().cloned().collect();managed.sort();
    let native_count=windows.values().filter(|window| window.hwnd().is_ok()).count();
    Ok(serde_json::json!({"managed":managed,"nativeCount":native_count,"inventoryScope":"Tauri-managed HWNDs, including hidden windows"}))
}
'''
    path.write_text(source)
    path=crate/'build.rs';source=path.read_text().replace('            "windows_smoke_report",','            "windows_smoke_report",\n            "smoke_windows",',1);path.write_text(source)
    path=crate/'src/navigation.rs';source=path.read_text();anchor='.window_features(features)'
    if source.count(anchor)!=1:raise RuntimeError('full UI hidden popout anchor changed')
    source=source.replace(anchor,anchor+'.visible(false)',1)
    source=source.replace('.initialization_script(close_script)','.initialization_script(close_script).initialization_script(include_str!("windows-full-ui-init.js"))',1)
    path.write_text(source)
    # The UI's host-list query must not inspect the user's real SSH files.
    path=crate/'src/remote.rs';source=path.read_text()
    if source.count('hosts::default_ssh_dir().as_deref()')!=1:raise RuntimeError('full UI private SSH inventory anchor changed')
    source=source.replace('hosts::default_ssh_dir().as_deref()','Some(std::path::Path::new('+json.dumps(common.windows(root/'empty-ssh'))+'))')
    (root/'empty-ssh').mkdir()
    path.write_text(source)

@contextlib.contextmanager
def approved_private_wsl(permit):
    """CLI opt-in only; direct/imported fixture helpers remain denied by default."""
    global WSL_EXECUTION_APPROVED
    if permit is not True:raise RuntimeError('explicit --disposable-distros opt-in required')
    if WSL_EXECUTION_APPROVED:raise RuntimeError('private WSL run already active')
    WSL_EXECUTION_APPROVED=True
    try:yield
    finally:WSL_EXECUTION_APPROVED=False

def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cache-workspace',type=Path,help='marked TEMP Cargo target artifacts only; fresh source/driver always copied')
    parser.add_argument('--full-ui',action='store_true',help='exercise real packaged frontend assets and bounded shared UI/popout checks')
    parser.add_argument('--disposable-distros',action='store_true',required=True,help='explicitly create, run and clean two isolated temporary distributions')
    args=parser.parse_args(argv)
    with approved_private_wsl(args.disposable_distros):
        _run_full(args)

def fixture_executable_path(root):
    # Convert only the fresh root: wslpath on the child path resolves the Cargo
    # target junction into its cache and would violate the exact launch guard.
    return str(PureWindowsPath(common.windows(root))/'target'/'debug'/'splitty-gui.exe')

def _run_full(args):
    if not WSL_EXECUTION_APPROVED:raise RuntimeError('explicit --disposable-distros scoped approval required')
    repo=HERE.parents[1]
    # Strip only the copied test payload to keep fixture provisioning bounded in
    # time; source/build provenance remains the current debug executable.
    temp=Path(subprocess.check_output(['wslpath','-u',common.powershell('$env:TEMP')],text=True).strip())/('splitty-native-windows-payload-'+uuid.uuid4().hex[:12]);temp.mkdir()
    payload=temp/'splitty'; subprocess.run(['strip','--strip-debug','-o',str(payload),str(repo/'target/debug/splitty')],check=True)
    root=common.copy_workspace(repo,payload);print(root,flush=True)
    (root/'payload-provenance.json').write_text(json.dumps({'sourceDebugSha256':hashlib.sha256((repo/'target/debug/splitty').read_bytes()).hexdigest(),'strippedTestSha256':hashlib.sha256(payload.read_bytes()).hexdigest()},indent=2))
    if args.cache_workspace:
        if not (args.cache_workspace/'TEST-ONLY.txt').is_file() or not args.cache_workspace.name.startswith('splitty-native-windows-'): raise RuntimeError('cache must be a marked temporary Windows fixture')
        common.powershell('New-Item -ItemType Junction -Path '+common.quote(common.windows(root/'target'))+' -Target '+common.quote(common.windows(args.cache_workspace.resolve()/'target'))+' | Out-Null')
    appdata=root/'appdata';appdata.mkdir()
    # Snapshot global networking configuration read-only; never edit it.
    global_config=common.powershell('$p=Join-Path $env:USERPROFILE ".wslconfig"; if(Test-Path $p){Get-Content $p}else{"<absent>"}')
    (root/'global-wslconfig-readonly.txt').write_text(global_config)
    with fixtures(root) as names:
        instrument(root,names,repo,full_ui=args.full_ui)
        (appdata/'windows.json').write_text(json.dumps({'selectedDistro':names[0]}))
        # Only the current Windows child process receives these fixture guards.
        original=common.powershell
        def scoped(source,log=None):
            prefix='$env:SPLITTY_SMOKE_APP_DATA='+common.quote(common.windows(appdata))+'\n$env:SPLITTY_SMOKE_DISTROS='+common.quote(json.dumps(names))+'\n'
            return original(prefix+source,log)
        common.powershell=scoped
        try:
            common.cargo(root,'build --locked -p splitty-gui --bin splitty-gui','windows-wsl-build.log')
            evidence=root/'runtime-evidence'
            driver=root/'wsl-runtime.ps1'; shutil.copyfile(HERE/'tauri-windows-native-smoke/wsl-runtime.ps1',driver)
            shutil.copyfile(HERE/'tauri-windows-native-smoke/fixture-job.cs',root/'fixture-job.cs')
            shutil.copyfile(HERE/'tauri-windows-native-smoke/fixture-sentinel.ps1',root/'fixture-sentinel.ps1')
            shutil.copyfile(HERE/'tauri-windows-native-smoke/fixture-guest-guard.sh',root/'fixture-guest-guard.sh')
            shutil.copyfile(HERE/'tauri-windows-native-smoke/fixture-registration.ps1',root/'fixture-registration.ps1')
            other=args.cache_workspace.resolve()/'target/debug/splitty-gui.exe' if args.cache_workspace else root/'target/debug/splitty-gui.exe'
            command='& '+common.quote(common.windows(driver))+' -Executable '+common.quote(fixture_executable_path(root))+' -OtherExecutable '+common.quote(common.windows(other))+' -RootDir '+common.quote(common.windows(root))+' -FixtureB '+common.quote(names[1])+' -PermitIsolatedFullRun\nexit $LASTEXITCODE'
            common.powershell(command,root/'windows-wsl-driver.log')
            process_evidence=json.loads((evidence/'processes.json').read_text(encoding='utf-8-sig'))
            if not process_evidence.get('IndependentFixturePreserved'):raise RuntimeError('independent fixture work did not survive app exit')
            guest=(evidence/'guest-live.txt').read_text(encoding='utf-8-sig')
            if 'PID:' not in guest or 'LISTEN:' in guest: raise RuntimeError('guest native owner TCP inventory failed')
            value=json.loads((evidence/'runtime-result.json').read_text()); path=value['savedPath']
            if not path.startswith('/home/splitty-fixture/'): raise RuntimeError('file saved outside isolated HOME')
            observed=wsl(names[1],['sha256sum',path]).split()[0]
            expected=hashlib.sha256(bytes(index%256 for index in range(1_048_833))).hexdigest()
            if observed!=expected: raise RuntimeError('WSL uploaded file digest mismatch')
            for name in names:
                tasks=wsl(name,['ps','-e','-o','comm=']).splitlines()
                if 'splitty' in [task.strip() for task in tasks]: raise RuntimeError('owned WSL backend survived app exit')
            if wsl(names[1],['sh','-c','kill -0 '+str(value['terminalPid'])+' 2>/dev/null; test $? -ne 0']).strip(): raise RuntimeError('unexpected shell cleanup output')
            (root/'wsl-verification.json').write_text(json.dumps({'uploadedSha256':observed,'terminalExited':True,'ownedBackendsExited':True,'unrelatedFixtureWorkPreserved':True,'guestTcpListeners':False,'realAppRestart':True,'names':names},indent=2))
            print(evidence,flush=True)
        finally:
            common.powershell=original

if __name__=='__main__': main()
