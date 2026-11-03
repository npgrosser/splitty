#!/usr/bin/env python3
"""Isolated native Windows OpenSSH host preflight; no user SSH files or GUI."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import threading
import time

HERE=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('wsl_fixture',HERE/'tauri-windows-wsl-smoke.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
common=fixture.common
SSH=Path('/mnt/c/Windows/System32/OpenSSH/ssh.exe')

def host_rootfs(path,base,keys,port):
    base(path)
    package=Path('/tmp/splitty-native-sshd/root')
    files={str(path.relative_to(package)):path.resolve() for path in (package/'usr/lib/x86_64-linux-gnu').glob('*.so*')}
    library_env={**os.environ,'LD_LIBRARY_PATH':str(package/'usr/lib/x86_64-linux-gnu')}
    for name in ['sshd','sshd-session','sshd-auth']:
        source=package/('usr/sbin/sshd' if name=='sshd' else 'usr/lib/openssh/'+name)
        files[str(source.relative_to(package))]=source
        for line in subprocess.check_output(['ldd',str(source)],text=True,env=library_env).splitlines():
            match=re.search(r'=> (/\S+)',line) or re.search(r'^\s*(/\S+)',line)
            if match:
                library=Path(match.group(1)); destination=str(library.relative_to(package)) if library.is_relative_to(package) else str(library).lstrip('/');files[destination]=library.resolve()
    for name in ['mktemp','ls','find','wc','cut','tr','head','tail','stat','chown','setsid','touch','date','sed','grep','env','dirname']:
        source=Path(shutil.which(name));files['usr/bin/'+name]=source.resolve()
        for line in subprocess.check_output(['ldd',str(source.resolve())],text=True).splitlines():
            match=re.search(r'=> (/\S+)',line) or re.search(r'^\s*(/\S+)',line)
            if match: files[match.group(1).lstrip('/')]=Path(match.group(1)).resolve()
    texts={
      'etc/passwd':'root:x:0:0:root:/root:/bin/sh\nsplitty-fixture:x:1000:1000:fixture:/home/splitty-fixture:/bin/sh\nsshd:x:101:101:sshd:/run/sshd:/bin/sh\n',
      'etc/group':'root:x:0:\nsplitty-fixture:x:1000:\nsshd:x:101:\n',
      'etc/shadow':'root:*:20000:0:99999:7:::\nsplitty-fixture::20000:0:99999:7:::\nsshd:*:20000:0:99999:7:::\n',
      'etc/sshd_config':f'Port {port}\nListenAddress 127.0.0.1\nHostKey /etc/fixture-hostkey\nAuthorizedKeysFile /etc/fixture-authorized_keys\nUsePAM no\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nAllowUsers splitty-fixture\nAllowTcpForwarding no\nPermitTunnel no\nX11Forwarding no\nPidFile /run/fixture-sshd.pid\nSshdSessionPath /usr/lib/openssh/sshd-session\nSshdAuthPath /usr/lib/openssh/sshd-auth\nForceCommand /usr/bin/fixture-host-env\nLogLevel VERBOSE\n',
      'usr/bin/fixture-host-env':'#!/bin/sh\nexport HOME=/home/splitty-fixture XDG_CONFIG_HOME=/home/splitty-fixture/.config XDG_DATA_HOME=/home/splitty-fixture/.local/share XDG_RUNTIME_DIR=/home/splitty-fixture/run PATH=/usr/bin:/bin\ncd "$HOME" || exit 1\nexec /bin/sh -c "$SSH_ORIGINAL_COMMAND"\n',
    }
    files['etc/fixture-hostkey']=keys/'hostkey';files['etc/fixture-authorized_keys']=keys/'clientkey.pub'
    with tarfile.open(path,'a') as archive:
        for name in ['usr/sbin','usr/lib/openssh','run/sshd']:
            info=tarfile.TarInfo(name);info.type=tarfile.DIRTYPE;info.mode=0o755;archive.addfile(info)
        for name,source in files.items():
            info=tarfile.TarInfo(name);info.size=source.stat().st_size;info.mode=0o600 if name=='etc/fixture-hostkey' else 0o755
            with source.open('rb') as data:archive.addfile(info,data)
        for name,text in texts.items():
            data=text.encode();info=tarfile.TarInfo(name);info.size=len(data);info.mode=0o755 if name.startswith('usr/bin/') else 0o600;archive.addfile(info,io.BytesIO(data))

def gui(root,config,names,repo,cache):
    raise RuntimeError("Windows remote GUI fixture disabled pending independent process-ownership cleanup review")
    common.instrument(root)
    crate=root/'crates/splitty-tauri'
    path=crate/'src/lib.rs';source=path.read_text().replace('.window("main").local(true).permission("allow-windows-smoke-report")','.window("*").local(true).permission("allow-windows-smoke-report")')
    source=source.replace('    app.exit(if failed { 1 } else { 0 });','    if value["phase"] != "restart" || failed { app.exit(if failed { 1 } else { 0 }); }')
    source=source.replace('.join("runtime-result.json"), serde_json::to_vec_pretty(&value)', '.join(if value["phase"] == "restart" { "checkpoint-restart.json" } else { "runtime-result.json" }), serde_json::to_vec_pretty(&value)')
    path.write_text(source)
    path=crate/'src/remote.rs';source=path.read_text().replace('    let _ = window.show();','    // Fixture only: keep remote window hidden.').replace('    let _ = window.set_focus();','    // Fixture only: never take user focus.')
    source=source.replace('    app.path()\n        .app_config_dir()\n        .map_err(|error| format!("could not resolve the configuration directory: {error}"))','    Ok(PathBuf::from('+json.dumps(common.windows(root/'appdata'))+'))')
    path.write_text(source)
    path=crate/'src/windows_wsl.rs';source=path.read_text().replace('fn config_path(app: &AppHandle) -> Result<PathBuf, String> {','fn config_path(app: &AppHandle) -> Result<PathBuf, String> { return Ok(PathBuf::from('+json.dumps(common.windows(root/'appdata/windows.json'))+')); #[allow(unreachable_code)]',1);path.write_text(source)
    path=root/'crates/splitty-remote/src/ssh.rs';source=path.read_text();anchor='.args(hardening_options())';assert source.count(anchor)==1
    source=source.replace(anchor,anchor+'.arg("-F").arg('+json.dumps(common.windows(config))+')',1);path.write_text(source)
    assets=crate/'runtime-assets'
    for name in ['index.html','setup.html']:(assets/name).write_text('<!doctype html><html><head><script type="module" src="/probe.js"></script></head><body>Hidden native remote fixture</body></html>')
    vite=root/'remote-vite.config.mjs';vite.write_text('import {defineConfig} from '+json.dumps(str(repo/'frontend/node_modules/vite/dist/node/index.js'))+'; export default defineConfig({build:{outDir:'+json.dumps(str(assets))+',emptyOutDir:false,minify:false,lib:{entry:'+json.dumps(str(HERE/'tauri-windows-native-smoke/remote-entry.ts'))+',name:"RemoteProbe",formats:["iife"],fileName:()=>"probe.js"},target:"es2022"}});')
    with (root/'remote-probe-build.log').open('w') as output:subprocess.run([str(repo/'frontend/node_modules/.bin/vite'),'build','--config',str(vite)],cwd=repo,stdout=output,stderr=subprocess.STDOUT,check=True)
    (root/'runtime-source-hashes.json').write_text(json.dumps(common.hashes(root),indent=2))
    if cache and (not (cache/'TEST-ONLY.txt').is_file() or not cache.name.startswith('splitty-native-windows-')):raise RuntimeError('unmarked cache workspace')
    if cache:common.powershell('New-Item -ItemType Junction -Path '+common.quote(common.windows(root/'target'))+' -Target '+common.quote(common.windows(cache/'target'))+' | Out-Null')
    common.cargo(root,'build --locked -p splitty-gui --bin splitty-gui','windows-remote-build.log')
    evidence=root/'runtime-evidence';stopped=threading.Event();errors=[]
    def controller():
        try:
            while not stopped.wait(.2):
                if not (evidence/'checkpoint-restart.json').exists():continue
                restart='set -- "$HOME"/.local/share/splitty/remote/*/splitty; "$1" daemon stop; sleep 2; "$1" daemon start'
                result=subprocess.run([str(SSH),'-F',common.windows(config),'splitty-native-windows-fixture',restart],capture_output=True,text=True,timeout=30)
                (root/'host-restart.json').write_text(json.dumps({'exitCode':result.returncode,'stdout':result.stdout,'stderr':result.stderr},indent=2))
                if result.returncode:raise RuntimeError('host restart failed')
                return
        except Exception as error:errors.append(str(error))
    thread=threading.Thread(target=controller,daemon=True);thread.start()
    original=common.powershell
    def scoped(source,log=None):return original('$env:SPLITTY_REMOTE_PAYLOAD='+common.quote(common.windows(root/'crates/splitty-tauri/resources/splitty-wsl-x86_64.tar.gz'))+'\n'+source,log)
    common.powershell=scoped
    try:common.runtime(root,max_seconds=120)
    finally:stopped.set();thread.join(35);common.powershell=original
    if errors:raise RuntimeError(errors)
    value=json.loads((evidence/'runtime-result.json').read_text());pid=int(value['terminalPid'])
    result=subprocess.run([str(SSH),'-F',common.windows(config),'splitty-native-windows-fixture',f'kill -0 {pid}; test ! -e "$HOME/replayed"'],capture_output=True,text=True,check=True)
    (root/'remote-after-close.json').write_text(json.dumps({'ptySurvivedAppClose':True,'inputNotReplayed':True,'pid':pid,'stdout':result.stdout,'stderr':result.stderr},indent=2))

def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--disposable-distros',action='store_true',required=True);parser.add_argument('--gui',action='store_true');parser.add_argument('--cache-workspace',type=Path);args=parser.parse_args()
    repo=HERE.parents[1];root=common.copy_workspace(repo,repo/'target/debug/splitty');print(root,flush=True)
    keys=root/'ssh';keys.mkdir()
    for name in ['hostkey','clientkey']:subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(keys/name)],check=True)
    port=int(common.powershell('$s=[System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback,0);$s.Start();$p=$s.LocalEndpoint.Port;$s.Stop();$p'))
    alias='splitty-native-windows-fixture'
    public=(keys/'hostkey.pub').read_text().split();(keys/'known_hosts').write_text(alias+' '+public[0]+' '+public[1]+'\n')
    config=keys/'config';config.write_text(f'Host {alias}\n HostName 127.0.0.1\n Port {port}\n User splitty-fixture\n HostKeyAlias {alias}\n IdentityFile "{common.windows(keys/"clientkey")}"\n UserKnownHostsFile "{common.windows(keys/"known_hosts")}"\n GlobalKnownHostsFile none\n IdentitiesOnly yes\n IdentityAgent none\n BatchMode yes\n StrictHostKeyChecking yes\n ConnectTimeout 5\n')
    common.powershell('& icacls.exe '+common.quote(common.windows(keys/'clientkey'))+' /inheritance:r /grant:r ($env:USERNAME+":F") | Out-Null; if($LASTEXITCODE){exit $LASTEXITCODE}')
    resolved=subprocess.run([str(SSH),'-F',common.windows(config),'-G',alias],capture_output=True,text=True,check=True)
    (root/'windows-ssh-resolved.txt').write_text(resolved.stdout)
    if 'userknownhostsfile '+common.windows(keys/'known_hosts').lower() not in resolved.stdout.lower(): raise RuntimeError('native SSH did not resolve isolated known_hosts')
    original=fixture.rootfs;fixture.rootfs=lambda path:host_rootfs(path,original,keys,port)
    with fixture.fixtures(root) as names:
        # The launcher, never the app, addresses only its recorded private host.
        command='& wsl.exe -d '+common.quote(names[0])+' --user root --exec /bin/sh -c '+common.quote('mkdir -p /run/sshd; exec /usr/sbin/sshd -f /etc/sshd_config -E /tmp/fixture-sshd.log')+'\nexit $LASTEXITCODE'
        common.powershell(command,root/'fixture-sshd-start.log')
        result=subprocess.run([str(SSH),'-F',common.windows(config),alias,'printf "HOST_HOME=%s\\n" "$HOME"; id -u'],capture_output=True,text=True)
        (root/'fixture-sshd.log').write_text(common.powershell('& wsl.exe -d '+common.quote(names[0])+' --user root --exec cat /tmp/fixture-sshd.log'))
        (root/'native-ssh-preflight.json').write_text(json.dumps({'stdout':result.stdout,'stderr':result.stderr,'exitCode':result.returncode,'target':alias,'port':port,'host':names[0]},indent=2))
        if result.returncode or 'HOST_HOME=/home/splitty-fixture' not in result.stdout or '1000' not in result.stdout:raise RuntimeError('native Windows SSH host preflight failed: '+result.stderr)
        print(root/'native-ssh-preflight.json',flush=True)
        if args.gui:gui(root,config,names,repo,args.cache_workspace)

if __name__=='__main__':main()
