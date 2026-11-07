; PeekAround Windows installer (per-user, like the Electron build's NSIS: oneClick false, perMachine false)
; makensis -DVERSION=2.0.0 -DSRC=<staged dir> -DOUT=<setup.exe> installer.nsi
Unicode true
!include "MUI2.nsh"
Name "PeekAround"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\PeekAround"
InstallDirRegKey HKCU "Software\PeekAround" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ICON "${ICON}"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\PeekAround.exe"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Turkish"
!insertmacro MUI_LANGUAGE "English"

Section "PeekAround"
  ; a running copy holds the files: ask it to quit first is not possible headless — just retry
  SetOutPath "$INSTDIR"
  File /r "${SRC}\*.*"
  WriteRegStr HKCU "Software\PeekAround" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortCut "$SMPROGRAMS\PeekAround.lnk" "$INSTDIR\PeekAround.exe"
  CreateShortCut "$DESKTOP\PeekAround.lnk" "$INSTDIR\PeekAround.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeekAround" "DisplayName" "PeekAround"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeekAround" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeekAround" "DisplayIcon" "$INSTDIR\PeekAround.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeekAround" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  ; user data (%APPDATA%\PeekAround: database, settings) is kept, like the Electron uninstaller
  Delete "$SMPROGRAMS\PeekAround.lnk"
  Delete "$DESKTOP\PeekAround.lnk"
  RMDir /r "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\PeekAround"
  DeleteRegKey HKCU "Software\PeekAround"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PeekAround"
SectionEnd
