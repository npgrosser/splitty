#!/usr/bin/env python3
"""Isolated Windows GPUI + owned WSL backend smoke. Run only with --execute."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import tempfile
import time


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def powershell(script, timeout=20):
    script = "$ErrorActionPreference = 'Stop'\n" + script
    encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
    # File handles avoid waiting for a GUI child that inherited PowerShell's
    # capture pipes after its launching shell has already exited.
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        run = subprocess.run(
            ["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
            stdout=out, stderr=err, timeout=timeout,
        )
        out.seek(0)
        err.seek(0)
        stdout = out.read().decode("utf-8", errors="replace")
        stderr = err.read().decode("utf-8", errors="replace")
    if run.returncode:
        raise RuntimeError(f"PowerShell failed: {stderr[-1200:]}")
    return stdout.strip()


def psquote(value):
    return "'" + str(value).replace("'", "''") + "'"


def windows_identity(ident):
    return f'''
$p = Get-Process -Id {ident["pid"]} -ErrorAction SilentlyContinue
if ($null -eq $p -or $p.StartTime.ToUniversalTime().Ticks -ne {ident["startTicks"]} -or $p.Path -ne {psquote(ident["path"])}) {{ throw 'owned Windows process identity changed' }}
'''


STARTUP_CAPTURE_SETUP = r'''
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class SplittyStartup {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  public static IntPtr VisibleWindow(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) { found = h; return false; } return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
'''

STARTUP_CAPTURE = r'''
$frames = @()
$firstVisible = $null
$captureError = $null
try {
  while ($timer.ElapsedMilliseconds -lt 10000 -and -not $p.HasExited) {
    $h = [SplittyStartup]::VisibleWindow($p.Id)
    if ($h -ne [IntPtr]::Zero) {
      $firstVisible = $timer.ElapsedMilliseconds
      foreach ($delay in @(0, 50, 200)) {
        while ($timer.ElapsedMilliseconds -lt ($firstVisible + $delay)) { Start-Sleep -Milliseconds 1 }
        $rect = New-Object SplittyStartup+Rect
        if (-not [SplittyStartup]::GetWindowRect($h, [ref]$rect)) { throw 'startup window vanished' }
        $width = $rect.Right - $rect.Left
        $height = $rect.Bottom - $rect.Top
        if ($width -lt 300 -or $height -lt 200) { throw 'startup window too small' }
        $foreground = [SplittyStartup]::GetForegroundWindow() -eq $h
        $bmp = New-Object System.Drawing.Bitmap($width, $height)
        $graphics = [System.Drawing.Graphics]::FromImage($bmp)
        $at = $timer.ElapsedMilliseconds
        $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
        $graphics.Dispose()
        $path = $out + '.startup-' + $delay + '.png'
        $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $bmp.Dispose()
        $frames += @{ atMs=$at; path=$path; foreground=$foreground; width=$width; height=$height }
      }
      break
    }
    Start-Sleep -Milliseconds 1
  }
  if ($null -eq $firstVisible) { throw 'no visible startup window within ten seconds' }
} catch { $captureError = $_.Exception.Message }
@{ firstVisibleMs=$firstVisible; frames=$frames; error=$captureError } |
  ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 $startupReport
'''


def start_windows(exe, arguments, capture_startup=False):
    # Start-Process exposes the real Windows PID, creation time, executable and
    # owned log paths. Linux's WSL interop proxy PID is never used for cleanup.
    script = "$ErrorActionPreference = 'Stop'\n"
    if capture_startup:
        script += STARTUP_CAPTURE_SETUP
    script += f'''
$out = Join-Path $env:TEMP ('splitty-preview-' + [Guid]::NewGuid().ToString('N') + '.stdout')
$err = $out + '.stderr'
$startupReport = $out + '.startup.json'
$timer = [System.Diagnostics.Stopwatch]::StartNew()
$p = Start-Process -FilePath {psquote(exe)} -ArgumentList {psquote(subprocess.list2cmdline(arguments))} -RedirectStandardOutput $out -RedirectStandardError $err -PassThru
$p.Refresh()
$null = $p.Handle
Write-Output ("SPLITTY_JSON " + (@{{ pid=$p.Id; startTicks=$p.StartTime.ToUniversalTime().Ticks; path=$p.Path; stdout=$out; stderr=$err; startupReport=$startupReport }} | ConvertTo-Json -Compress))
'''
    if capture_startup:
        script += STARTUP_CAPTURE
    script += '''
$p.WaitForExit()
Write-Output ("SPLITTY_EXIT " + $p.ExitCode)
'''
    encoded = base64.b64encode(script.encode("utf-16le")).decode("ascii")
    controller = subprocess.Popen(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    ready, _, _ = select.select([controller.stdout], [], [], 10)
    if not ready:
        controller.kill()
        controller.communicate(timeout=3)
        raise TimeoutError("Windows controller did not report its launched PID")
    line = controller.stdout.readline().strip()
    if not line.startswith("SPLITTY_JSON "):
        controller.kill()
        _, stderr = controller.communicate(timeout=3)
        raise RuntimeError(f"Windows controller failed before ownership report: {line} {stderr[-800:]}")
    return json.loads(line.removeprefix("SPLITTY_JSON ")), controller


def collect_startup(ident, output, prefix, temp_files):
    report_path = linux_path(ident["startupReport"])
    report = json.loads(report_path.read_text(encoding="utf-8-sig"))
    shutil.copyfile(report_path, output / f"{prefix}-startup.json")
    temp_files.append(report_path)
    for index, frame in enumerate(report["frames"]):
        path = linux_path(frame["path"])
        shutil.copyfile(path, output / f"{prefix}-startup-{index}.png")
        temp_files.append(path)
    if report["error"] or len(report["frames"]) != 3:
        raise RuntimeError(f"startup capture failed: {report}")
    if not all(frame["foreground"] for frame in report["frames"]):
        raise RuntimeError("startup screenshots were obscured; visual evidence is inconclusive")
    return report


def finish_windows(controller, seconds):
    stdout, stderr = controller.communicate(timeout=seconds)
    if controller.returncode:
        raise RuntimeError(f"Windows controller failed: {stderr[-1000:]}")
    values = [line.removeprefix("SPLITTY_EXIT ") for line in stdout.splitlines()
              if line.startswith("SPLITTY_EXIT ")]
    if len(values) != 1:
        raise RuntimeError(f"Windows controller omitted exit code: {stdout[-500:]} {stderr[-500:]}")
    # Windows PowerShell Start-Process returns a Process object whose ExitCode
    # is blank even after WaitForExit (reproduced with a controlled exit 7).
    # The app's structured probe and shutdown records are the runtime gate.
    return int(values[0]) if values[0] else None


def psjson(script):
    for line in powershell(script).splitlines():
        if line.startswith("SPLITTY_JSON "):
            return json.loads(line.removeprefix("SPLITTY_JSON "))
    raise RuntimeError("PowerShell did not return ownership evidence")


def windows_alive(ident):
    script = f'''
$p = Get-Process -Id {ident["pid"]} -ErrorAction SilentlyContinue
$alive = $null -ne $p -and $p.StartTime.ToUniversalTime().Ticks -eq {ident["startTicks"]} -and $p.Path -eq {psquote(ident["path"])}
Write-Output ("SPLITTY_JSON " + (@{{ alive=$alive }} | ConvertTo-Json -Compress))
'''
    return psjson(script)["alive"]


def window_action(action, ident):
    # Validate the process identity on every action. The window context title is
    # only a secondary check; a foreign Windows preview is never targeted.
    script = r'''
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class SplittyWindow {
  [StructLayout(LayoutKind.Sequential)] public struct Rect {
    public int Left, Top, Right, Bottom;
  }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
"@
'''
    script += windows_identity(ident)
    script += r'''
$title = $p.MainWindowTitle
if ($title -ne 'Splitty · Windows preview' -and $title -ne 'Splitty' -and -not $title.EndsWith(' · Splitty', [StringComparison]::Ordinal)) { throw 'owned process window title mismatch' }
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { throw 'preview has no window handle' }
if (-not [SplittyWindow]::SetForegroundWindow($h)) { throw 'cannot focus preview' }
Start-Sleep -Milliseconds 200
if ([SplittyWindow]::GetForegroundWindow() -ne $h) { throw 'preview is not foreground' }
$rect = New-Object SplittyWindow+Rect
if (-not [SplittyWindow]::GetWindowRect($h, [ref]$rect)) { throw 'cannot measure window' }
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -lt 300 -or $height -lt 200) { throw 'preview window too small' }
'''
    if action in ("before", "library", "input"):
        if action == "library":
            script += "[System.Windows.Forms.SendKeys]::SendWait('%p')\nStart-Sleep -Milliseconds 700\n"
        if action == "input":
            script += "[System.Windows.Forms.SendKeys]::SendWait('echo LATE_OK{ENTER}')\nStart-Sleep -Milliseconds 900\n"
        script += r'''
$path = Join-Path $env:TEMP ('splitty-native-preview-' + [Guid]::NewGuid().ToString('N') + '.png')
$bmp = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bmp)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bmp.Size)
$graphics.Dispose()
$bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output ("SPLITTY_JSON " + (@{ pid=$p.Id; path=$path; width=$width; height=$height } | ConvertTo-Json -Compress))
'''
    elif action == "close":
        script += r'''
[System.Windows.Forms.SendKeys]::SendWait('{ESC}')
Start-Sleep -Milliseconds 200
if (-not [SplittyWindow]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) {
  throw 'WM_CLOSE failed'
}
Write-Output ("SPLITTY_JSON " + (@{ pid=$p.Id; closed=$true } | ConvertTo-Json -Compress))
'''
    else:
        raise ValueError(action)
    return psjson(script)


def stop_windows(ident):
    if not windows_alive(ident):
        return
    script = windows_identity(ident) + f"Stop-Process -Id {ident['pid']} -Force\n"
    powershell(script)


def wait_windows_exit(ident, seconds):
    deadline = time.monotonic() + seconds
    while windows_alive(ident):
        if time.monotonic() >= deadline:
            raise TimeoutError(f"owned Windows process {ident['pid']} did not exit")
        time.sleep(.3)


def linux_path(windows_path):
    return Path(subprocess.check_output(["wslpath", "-u", windows_path], text=True).strip())


def owned_linux_pids(config):
    value = f"SPLITTY_CONFIG_DIR={config}".encode()
    result = set()
    for proc in Path("/proc").iterdir():
        if not proc.name.isdecimal():
            continue
        try:
            env = (proc / "environ").read_bytes().split(b"\0")
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        if value in env:
            result.add(int(proc.name))
    return result


def owned_linux_processes(config):
    result = []
    for pid in sorted(owned_linux_pids(config)):
        try:
            command = Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        result.append({"pid": pid, "command": command[:500]})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--backend", type=Path, required=True)
    parser.add_argument("--distro", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--capture-startup", action="store_true",
                        help="Capture the first observed visible frame and two following frames")
    parser.add_argument("--theme", choices=("splitty-dark", "splitty-light"),
                        help="Seed the isolated backend appearance for startup color review")
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    args.exe = args.exe.resolve()
    args.backend = args.backend.resolve()
    args.output = args.output.resolve()
    if not args.exe.is_file() or not args.backend.is_file():
        parser.error("EXE and Linux backend must exist")
    if not args.execute:
        print("Prepared only. Add --execute after the matching backend and EXE are frozen.")
        return
    args.output.mkdir(parents=True, exist_ok=False)
    config = args.output / "config"
    templates = config / "templates"
    templates.mkdir(parents=True, mode=0o700)
    if args.theme:
        (config / "config.toml").write_text(
            f'[appearance]\ntheme_mode = "fixed"\ntheme = "{args.theme}"\n'
        )
    (templates / "windows-smoke.yaml").write_text(json.dumps({
        "name": "Windows template proof",
        "panes": {"one": {"dir": "."}},
        "layout": {"slot": "one"},
    }) + "\n")
    command = [str(args.exe), "--wsl-distro", args.distro,
               "--wsl-backend", str(args.backend), "--wsl-config-dir", str(config)]
    result = {
        "exe": str(args.exe), "exeSha256": digest(args.exe),
        "backend": str(args.backend), "backendSha256": digest(args.backend),
        "distro": args.distro, "config": str(config), "checks": [],
    }
    sentinel = subprocess.Popen(["sleep", "180"], start_new_session=True)
    owned_windows = []
    controllers = []
    temp_files = []
    before = set()
    try:
        before = owned_linux_pids(config)
        win_exe = subprocess.check_output(["wslpath", "-w", str(args.exe)], text=True).strip()
        launch_args = command[1:]
        smoke, smoke_controller = start_windows(
            win_exe, launch_args + ["--workspace-smoke"], args.capture_startup,
        )
        owned_windows.append(smoke)
        controllers.append(smoke_controller)
        result["smokeWindowsIdentity"] = smoke
        time.sleep(3)
        result["stage4LinuxProcesses"] = owned_linux_processes(config)
        result["stage4ControlSockets"] = [str(path) for path in (config / "run").glob("*.sock")]
        if windows_alive(smoke):
            try:
                diagnostic = window_action("before", smoke)
                diagnostic_path = linux_path(diagnostic["path"])
                shutil.copyfile(diagnostic_path, args.output / "windows-smoke-stage4.png")
                temp_files.append(diagnostic_path)
                result["stage4Screenshot"] = str(args.output / "windows-smoke-stage4.png")
                print(json.dumps({"stage4Screenshot": result["stage4Screenshot"]}), flush=True)
            except Exception as diagnostic_error:
                result["stage4ScreenshotError"] = repr(diagnostic_error)
        smoke_exit = finish_windows(smoke_controller, 60)
        result["workspaceSmokeExit"] = smoke_exit
        if args.capture_startup:
            result["coldStartup"] = collect_startup(smoke, args.output, "cold", temp_files)
        for name in ("stdout", "stderr"):
            path = linux_path(smoke[name])
            shutil.copyfile(path, args.output / f"workspace.{name}")
            temp_files.append(path)
        lines = ((args.output / "workspace.stdout").read_text(errors="replace") + "\n"
                 + (args.output / "workspace.stderr").read_text(errors="replace")).splitlines()
        records = []
        for line in lines:
            try:
                records.append(json.loads(line))
            except json.JSONDecodeError:
                continue
        probes = [item for item in records if item.get("probe") == "splitty-local-workspace"]
        shutdowns = [item for item in records if item.get("probeShutdown") == "complete"]
        graphics = [item["nativeGraphics"] for item in records if "nativeGraphics" in item]
        if smoke_exit not in (None, 0) or len(probes) != 1 or probes[0].get("passed") is not True or probes[0].get("stage") != 18:
            raise RuntimeError("Windows workspace smoke did not pass all 18 stages")
        if len(shutdowns) != 1:
            raise RuntimeError("Windows workspace did not report complete shutdown")
        if not graphics or not graphics[-1].get("device") or graphics[-1].get("software") is True:
            raise RuntimeError("Windows GPUI did not report a hardware graphics device")
        result["workspaceProbe"] = probes[0]
        result["nativeGraphics"] = graphics[-1]
        result["checks"].append("windows-workspace-18-stages-hardware-gpu-and-shutdown")
        deadline = time.monotonic() + 5
        while owned_linux_pids(config) - before and time.monotonic() < deadline:
            time.sleep(.2)
        if owned_linux_pids(config) - before:
            raise RuntimeError("workspace smoke left an owned WSL process running")
        result["checks"].append("smoke-owned-wsl-backend-and-shells-reaped")
        app, app_controller = start_windows(win_exe, launch_args, args.capture_startup)
        owned_windows.append(app)
        controllers.append(app_controller)
        result["windowWindowsIdentity"] = app
        deadline = time.monotonic() + 20
        while True:
            try:
                first = window_action("before", app)
                break
            except RuntimeError:
                if time.monotonic() > deadline or not windows_alive(app):
                    raise
                time.sleep(.3)
        first_path = linux_path(first["path"])
        shutil.copyfile(first_path, args.output / "windows-workspace.png")
        temp_files.append(first_path)
        second = window_action("library", app)
        second_path = linux_path(second["path"])
        shutil.copyfile(second_path, args.output / "windows-template-library.png")
        temp_files.append(second_path)
        if digest(args.output / "windows-workspace.png") == digest(args.output / "windows-template-library.png"):
            raise RuntimeError("Alt+P did not change Windows preview pixels")
        result["windowsPid"] = app["pid"]
        result["screenshotSize"] = [first["width"], first["height"]]
        result["checks"].append("owned-windows-window-and-template-library-pixel-change")
        window_action("close", app)
        app_exit = finish_windows(app_controller, 15)
        if args.capture_startup:
            result["warmStartup"] = collect_startup(app, args.output, "warm", temp_files)
        if app_exit not in (None, 0):
            raise RuntimeError(f"normal Windows close returned {app_exit}")
        result["checks"].append("normal-windows-window-close")
        deadline = time.monotonic() + 10
        while owned_linux_pids(config) - before and time.monotonic() < deadline:
            time.sleep(.2)
        if owned_linux_pids(config) - before:
            raise RuntimeError("owned WSL backend or shell survived Windows close")
        result["checks"].append("owned-wsl-backend-and-shells-reaped")
        if sentinel.poll() is not None:
            raise RuntimeError("unrelated WSL sentinel exited during Windows close")
        result["checks"].append("unrelated-wsl-sentinel-preserved")
        result["passed"] = True
    except Exception as error:
        result["passed"] = False
        result["error"] = repr(error)
    finally:
        for owned in reversed(owned_windows):
            try:
                stop_windows(owned)
            except Exception as cleanup_error:
                result.setdefault("cleanupErrors", []).append(repr(cleanup_error))
        for controller in controllers:
            if controller.poll() is None:
                try:
                    finish_windows(controller, 5)
                except Exception as cleanup_error:
                    controller.kill()
                    controller.communicate(timeout=3)
                    result.setdefault("cleanupErrors", []).append(repr(cleanup_error))
        # Failure cleanup is limited to WSL processes carrying this run's
        # private configuration marker. Never signal a distro or a process name.
        for pid in owned_linux_pids(config):
            if pid not in before:
                try:
                    os.kill(pid, 15)
                except ProcessLookupError:
                    pass
        deadline = time.monotonic() + 5
        while owned_linux_pids(config) - before and time.monotonic() < deadline:
            time.sleep(.2)
        if sentinel.poll() is None:
            sentinel.terminate()
            sentinel.wait(timeout=3)
        result["sentinelPid"] = sentinel.pid
        for path in temp_files:
            path.unlink(missing_ok=True)
        result["ownedLinuxPidsAfter"] = sorted(owned_linux_pids(config))
        if result["ownedLinuxPidsAfter"] or result.get("cleanupErrors"):
            result["passed"] = False
        (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"passed": result["passed"], "result": str(args.output / "result.json")}))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
