#!/usr/bin/env python3
"""Build a minimal, digest-recorded Linux fixture from the hosted Ubuntu image."""
import argparse
import hashlib
import io
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile

TOOLS = ['sh', 'id', 'uname', 'printenv', 'cat', 'sleep']

def build(output):
    output.mkdir(parents=True, exist_ok=False)
    executable = output / 'fixture-net'
    subprocess.run(['gcc', '-static', '-O2', '-Wall', '-Wextra', '-Werror',
                    str(Path(__file__).with_name('net.c')), '-o', str(executable)], check=True)
    files = {'usr/bin/fixture-net': executable}
    for name in TOOLS:
        source = Path(shutil.which(name)).resolve()
        files['usr/bin/' + name] = source
        for line in subprocess.check_output(['ldd', str(source)], text=True).splitlines():
            match = re.search(r'=> (/\S+)', line) or re.search(r'^\s*(/\S+)', line)
            if match:
                path = Path(match[1])
                files[str(path).lstrip('/')] = path.resolve()
    texts = {
        'etc/passwd': 'root:x:0:0:root:/root:/bin/sh\nsplitty-fixture:x:1000:1000:fixture:/home/splitty-fixture:/bin/sh\n',
        'etc/group': 'root:x:0:\nsplitty-fixture:x:1000:\n',
        'etc/nsswitch.conf': 'passwd: files\ngroup: files\nhosts: files\n',
        'etc/wsl.conf': '[boot]\nsystemd=false\n[automount]\nenabled=false\nmountFsTab=false\n[interop]\nenabled=false\nappendWindowsPath=false\n[user]\ndefault=splitty-fixture\n',
        'etc/os-release': 'ID=splitty-test\nNAME="Disposable hosted WSL preflight"\n',
    }
    archive = output / 'rootfs.tar'
    with tarfile.open(archive, 'w') as tar:
        for directory in ['usr', 'usr/bin', 'etc', 'tmp', 'dev', 'proc', 'sys', 'run', 'root', 'home', 'home/splitty-fixture']:
            info = tarfile.TarInfo(directory); info.type = tarfile.DIRTYPE
            info.mode = 0o1777 if directory == 'tmp' else 0o755
            if directory == 'home/splitty-fixture': info.uid = info.gid = 1000; info.mode = 0o700
            tar.addfile(info)
        info = tarfile.TarInfo('bin'); info.type = tarfile.SYMTYPE; info.linkname = 'usr/bin'; tar.addfile(info)
        for name, source in sorted(files.items()):
            info = tarfile.TarInfo(name); info.mode = 0o755; info.size = source.stat().st_size
            with source.open('rb') as data: tar.addfile(info, data)
        for name, value in texts.items():
            data = value.encode(); info = tarfile.TarInfo(name); info.mode = 0o644; info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    manifest = {'sourceSha': os.environ.get('GITHUB_SHA'), 'runnerImage': os.environ.get('ImageVersion'),
                'rootfsSha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
                'netSha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
                'tools': {name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in files.items()}}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')

def build_application(output,ssh_package_root):
    """Reuse the existing full fixture's rootfs, preserving its exact tool set."""
    here = Path(__file__).resolve().parent
    repo = here.parents[2]
    spec = importlib.util.spec_from_file_location('wsl_fixture', here.parent/'tauri-windows-wsl-smoke.py')
    fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(fixture)
    binary = repo/'target/debug/splitty'
    subprocess.run(['strip','--strip-debug','-o',str(output/'splitty'),str(binary)],check=True)
    fixture.rootfs(output/'app-rootfs.tar')
    ssh_spec=importlib.util.spec_from_file_location('ssh_fixture',here.parent/'tauri-windows-remote-smoke.py')
    ssh_fixture=importlib.util.module_from_spec(ssh_spec);ssh_spec.loader.exec_module(ssh_fixture)
    if not ssh_package_root or not (ssh_package_root/'usr/sbin/sshd').is_file():raise RuntimeError('explicit staged SSH package root required')
    ssh=ssh_fixture.host_rootfs(output/'app-ssh-rootfs.tar',lambda path:shutil.copyfile(output/'app-rootfs.tar',path),None,1024,package=ssh_package_root)
    with tarfile.open(output/'frontend-dist.tar','w') as archive:
        for path in sorted((repo/'frontend/dist').rglob('*')):
            if path.is_file(): archive.add(path,arcname=str(path.relative_to(repo/'frontend/dist')),recursive=False)
    value = {'sourceSha':os.environ['GITHUB_SHA'],
             'productBase':'9c49e83ee9257a74302bf55168d3f03358dbbc5b', 'runnerImage':os.environ.get('ImageVersion'),
             'cargoLockSha256':hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
             'sourceDebugSha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'sshHelpers':ssh['helpers'],'sshTools':ssh['tools'],
             'sshPackages':{path.name:hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(ssh_package_root.parent.glob('*.deb'))},
             'files':{name:hashlib.sha256((output/name).read_bytes()).hexdigest()
                      for name in ['splitty','app-rootfs.tar','frontend-dist.tar','app-ssh-rootfs.tar']}}
    (output/'app-manifest.json').write_text(json.dumps(value,indent=2)+'\n')

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('output', type=Path); parser.add_argument('--application',action='store_true'); parser.add_argument('--ssh-package-root',type=Path)
    args=parser.parse_args(); build(args.output)
    if args.application: build_application(args.output,args.ssh_package_root)
