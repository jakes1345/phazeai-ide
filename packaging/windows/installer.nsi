; PhazeAI IDE Windows installer (NSIS).
; Built by .github/workflows/release.yml:
;   makensis /DAPP_VERSION=<version> /DBIN_DIR=<dir with .exe files> /DOUT_DIR=<output dir> installer.nsi

!ifndef APP_VERSION
  !error "Pass /DAPP_VERSION=<version>"
!endif
!ifndef BIN_DIR
  !define BIN_DIR "..\..\bin"
!endif
!ifndef OUT_DIR
  !define OUT_DIR "."
!endif

!define APP_NAME "PhazeAI IDE"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PhazeAI IDE"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_DIR}\phazeai-ide-${APP_VERSION}-windows-x64-setup.exe"
InstallDir "$PROGRAMFILES64\PhazeAI IDE"
RequestExecutionLevel admin
Unicode True

Section "Install"
  SetOutPath "$INSTDIR"
  File "${BIN_DIR}\phazeai-ui.exe"
  File "${BIN_DIR}\phazeai.exe"
  CreateShortcut "$DESKTOP\PhazeAI IDE.lnk" "$INSTDIR\phazeai-ui.exe"
  CreateDirectory "$SMPROGRAMS\PhazeAI IDE"
  CreateShortcut "$SMPROGRAMS\PhazeAI IDE\PhazeAI IDE.lnk" "$INSTDIR\phazeai-ui.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME} ${APP_VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "PhazeAI"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\phazeai-ui.exe"
  Delete "$INSTDIR\phazeai.exe"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$DESKTOP\PhazeAI IDE.lnk"
  Delete "$SMPROGRAMS\PhazeAI IDE\PhazeAI IDE.lnk"
  RMDir "$SMPROGRAMS\PhazeAI IDE"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
SectionEnd
