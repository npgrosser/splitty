; Per-user installer of the native Splitty app. Built by bundle.ps1, which
; passes VERSION, NUMERIC_VERSION, EXECUTABLE, PAYLOAD, CHECKSUM, ICON, LICENSE
; and OUTFILE.
;
; It keeps the product name, install directory, Start menu shortcut and
; uninstall entry of the earlier web app and removes that app's executable, so
; an update replaces it in place. Settings and the WSL backend are not touched.

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define PRODUCT "Splitty"
!define MAIN_BINARY "Splitty.exe"
!define LEGACY_BINARY "splitty-gui.exe"
!define INSTALL_KEY "Software\splitty\Splitty"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Splitty"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\${PRODUCT}"
InstallDirRegKey HKCU "${INSTALL_KEY}" ""
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${NUMERIC_VERSION}"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "FileDescription" "${PRODUCT} Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT License"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${MAIN_BINARY}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Deletes a program file; while the app runs the file is locked, so ask to
; close it and retry. A silent installation fails instead of waiting.
!macro RemoveProgram PATH
  !define ID ${__LINE__}
  retry_${ID}:
    ClearErrors
    Delete "${PATH}"
    IfErrors 0 done_${ID}
    IfSilent 0 +3
      SetErrorLevel 2
      Abort "${PRODUCT} is running."
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "${PRODUCT} is running. Close it, then choose Retry." IDRETRY retry_${ID}
    Abort "${PRODUCT} is running."
  done_${ID}:
  !undef ID
!macroend

Section "Install"
  !insertmacro RemoveProgram "$INSTDIR\${LEGACY_BINARY}"
  !insertmacro RemoveProgram "$INSTDIR\${MAIN_BINARY}"

  SetOutPath "$INSTDIR"
  File "/oname=${MAIN_BINARY}" "${EXECUTABLE}"
  File "/oname=LICENSE.txt" "${LICENSE}"
  SetOutPath "$INSTDIR\resources"
  File "/oname=splitty-wsl-x86_64.tar.gz" "${PAYLOAD}"
  File "/oname=splitty-wsl-x86_64.tar.gz.sha256" "${CHECKSUM}"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${MAIN_BINARY}"
  ; A desktop shortcut of the earlier app keeps working.
  IfFileExists "$DESKTOP\${PRODUCT}.lnk" 0 +2
    CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${MAIN_BINARY}"

  WriteRegStr HKCU "${INSTALL_KEY}" "" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$\"$INSTDIR\${MAIN_BINARY}$\""
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "splitty"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$\"$INSTDIR$\""
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr HKCU "${UNINSTALL_KEY}" "MainBinaryName" "${MAIN_BINARY}"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
  !insertmacro RemoveProgram "$INSTDIR\${MAIN_BINARY}"
  Delete "$INSTDIR\${LEGACY_BINARY}"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\resources\splitty-wsl-x86_64.tar.gz"
  Delete "$INSTDIR\resources\splitty-wsl-x86_64.tar.gz.sha256"
  RMDir "$INSTDIR\resources"
  Delete "$INSTDIR\uninstall.exe"
  ; Only when empty: other tools keep caches below this directory.
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "${INSTALL_KEY}"
  DeleteRegKey /ifempty HKCU "Software\splitty"
SectionEnd
