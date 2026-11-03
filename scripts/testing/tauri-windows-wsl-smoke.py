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
from pathlib import Path
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

def wsl(name, arguments, log=None):
    command='& wsl.exe -d '+common.quote(name)+' --exec '+' '.join(common.quote(argument) for argument in arguments)+'\nexit $LASTEXITCODE'
    return common.powershell(command, log)

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
def fixtures(root):
    names=['splitty-native-fixture-'+uuid.uuid4().hex[:12]+suffix for suffix in ['-a','-b']]
    before=registrations(); owned={}; archive=root/'fixture-rootfs.tar'; rootfs(archive)
    (root/'owned-distros.json').write_text(json.dumps({'before':before,'created':owned,'names':names},indent=2))
    try:
        for name in names:
            if name in registrations() or name in before: raise RuntimeError('fixture name already registered')
            location=root/'distributions'/name; location.mkdir(parents=True)
            owned[name]=common.windows(location)
            # Persist ownership before invoking import; finally also handles a partial import.
            (root/'owned-distros.json').write_text(json.dumps({'before':before,'created':owned,'names':names},indent=2))
            common.powershell('& wsl.exe --import '+common.quote(name)+' '+common.quote(common.windows(location))+' '+common.quote(common.windows(archive))+' --version 2\nexit $LASTEXITCODE',root/(name+'-import.log'))
            actual=wsl(name,['/usr/bin/id','-u']).strip()
            if actual!='1000': raise RuntimeError('fixture default user is not non-root: '+repr(actual))
            home=wsl(name,['/usr/bin/printenv','HOME']).strip()
            if home!='/home/splitty-fixture': raise RuntimeError('fixture HOME not isolated: '+repr(home))
        yield names
    finally:
        sentinel_file=root/'sentinels.json'
        if sentinel_file.exists(): stop_sentinels(json.loads(sentinel_file.read_text()))
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
        (root/'distro-cleanup.json').write_text(json.dumps(cleanup,indent=2))
        if any(row.get('error') or not row.get('unregistered') for row in cleanup): raise RuntimeError('fixture cleanup needs inspection: '+str(root/'distro-cleanup.json'))

def sentinels(root,names):
    records=[]
    for index,name in enumerate(names):
        command='wsl.exe -d '+name+' --exec sleep 600'
        source='$startup=New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ShowWindow=[uint16]0}; $created=Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{CommandLine='+common.quote(command)+';ProcessStartupInformation=$startup}; if($created.ReturnValue -ne 0){throw "fixture sentinel creation failed"}; $p=Get-CimInstance Win32_Process -Filter ("ProcessId="+$created.ProcessId); [ordered]@{Id=$p.ProcessId;Creation=$p.CreationDate.ToUniversalTime().ToString("o");Distro='+common.quote(name)+'} | ConvertTo-Json -Compress'
        records.append(json.loads(common.powershell(source)))
        (root/'sentinels.json').write_text(json.dumps(records,indent=2))
    return records

def verify_sentinels(records):
    for record in records:
        source='$p=Get-CimInstance Win32_Process -Filter '+common.quote('ProcessId='+str(record['Id']))+'; if(-not $p -or $p.CreationDate.ToUniversalTime().ToString("o") -ne '+common.quote(record['Creation'])+' -or $p.CommandLine -notlike '+common.quote('*'+record['Distro']+'*')+'){throw "independent fixture work did not survive"}'
        common.powershell(source)

def stop_sentinels(records):
    raise RuntimeError("Windows fixture process cleanup disabled pending independent ownership review")
    for record in records:
        source='$p=Get-CimInstance Win32_Process -Filter '+common.quote('ProcessId='+str(record['Id']))+'; if($p -and $p.CreationDate.ToUniversalTime().ToString("o") -eq '+common.quote(record['Creation'])+' -and $p.CommandLine -like '+common.quote('*'+record['Distro']+'*')+'){Stop-Process -Id '+str(record['Id'])+' -Force}'
        common.powershell(source)

def instrument(root,names,repo):
    common.instrument(root)
    crate=root/'crates/splitty-tauri'; path=crate/'src/windows_wsl.rs'; source=path.read_text()
    source=source.replace('fn config_path(app: &AppHandle) -> Result<PathBuf, String> {','fn config_path(app: &AppHandle) -> Result<PathBuf, String> {\n        if let Some(path) = std::env::var_os("SPLITTY_SMOKE_APP_DATA") { return Ok(PathBuf::from(path).join("windows.json")); }',1)
    anchor='''            self.check_cancelled()?;
            let mut command = wsl_command();'''
    shim='''            self.check_cancelled()?;
            if let Some(names) = std::env::var_os("SPLITTY_SMOKE_DISTROS") {
                let names: Vec<String> = serde_json::from_str(&names.to_string_lossy()).map_err(|e| e.to_string())?;
                if arguments == [OsString::from("--list"), OsString::from("--verbose")] {
                    return Ok(ProcessOutput { success:true, stdout:names.iter().map(|name| format!("  {name} Running 2\\n")).collect::<String>().into_bytes(), stderr:Vec::new() });
                }
                if arguments.first().map(|value| value == "-d") != Some(true) || !arguments.get(1).is_some_and(|value| names.iter().any(|name| value == name.as_str())) { return Err("fixture guard rejected an unowned WSL target".into()); }
            }
            let mut command = wsl_command();'''
    if source.count(anchor)!=1: raise RuntimeError('WSL executor instrumentation anchor changed')
    source=source.replace(anchor,shim,1); path.write_text(source)
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
    config=root/'wsl-vite.config.mjs'
    config.write_text('import {defineConfig} from '+json.dumps(str(repo/'frontend/node_modules/vite/dist/node/index.js'))+'; export default defineConfig({build:{outDir:'+json.dumps(str(assets))+',emptyOutDir:false,minify:false,lib:{entry:'+json.dumps(str(HERE/'tauri-windows-native-smoke/wsl-entry.ts'))+',name:"WslNativeProbe",formats:["iife"],fileName:()=>"probe.js"},target:"es2022"},define:{__FIXTURES__:'+json.dumps(json.dumps(names))+'}});')
    with (root/'wsl-probe-bundle.log').open('w') as output:
        subprocess.run([str(repo/'frontend/node_modules/.bin/vite'),'build','--config',str(config)],cwd=repo,stdout=output,stderr=subprocess.STDOUT,check=True)
    (root/'runtime-source-hashes.json').write_text(json.dumps(common.hashes(root),indent=2))

def main():
    raise RuntimeError("Windows WSL GUI fixture disabled pending independent process-ownership cleanup review")
    parser=argparse.ArgumentParser(description=__doc__); parser.add_argument('--cache-workspace',type=Path); parser.add_argument('--disposable-distros',action='store_true',required=True,help='explicitly enable creation of two guarded temporary distributions'); args=parser.parse_args()
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
        workers=sentinels(root,names)
        instrument(root,names,repo)
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
            other=args.cache_workspace.resolve()/'target/debug/splitty-gui.exe' if args.cache_workspace else root/'target/debug/splitty-gui.exe'
            command='& '+common.quote(common.windows(driver))+' -Executable '+common.quote(common.windows(root/'target/debug/splitty-gui.exe'))+' -OtherExecutable '+common.quote(common.windows(other))+' -RootDir '+common.quote(common.windows(root))+' -FixtureB '+common.quote(names[1])+'\nexit $LASTEXITCODE'
            common.powershell(command,root/'windows-wsl-driver.log')
            verify_sentinels(workers)
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
            stop_sentinels(workers)

if __name__=='__main__': main()
