#!/usr/bin/env python3
"""Build a minimal, digest-recorded Linux fixture from the hosted Ubuntu image."""
import argparse
import hashlib
import io
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

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument('output', type=Path)
    build(parser.parse_args().output)
