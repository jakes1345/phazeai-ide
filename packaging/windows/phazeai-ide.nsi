; PhazeAI IDE Windows installer.
; Built by .github/workflows/release.yml from the repository root:
;   makensis /DAPP_VERSION=<version> packaging\windows\phazeai-ide.nsi
; Expects bin\phazeai-ui.exe and bin\phazeai.exe relative to the working
; directory; writes the setup .exe next to them.

!define APP_NAME "PhazeAI IDE"
!ifndef APP_VERSION
  !error "Pass the version: makensis /DAPP_VERSION=1.2.3 phazeai-ide.nsi"
!endif
!define INSTALL_DIR "$PROGRAMFILES64\PhazeAI IDE"
Name "${APP_NAME} ${APP_VERSION}"
OutFile "phazeai-ide-${APP_VERSION}-windows-x64-setup.exe"
InstallDir "${INSTALL_DIR}"
RequestExecutionLevel admin
Unicode True

Section "Install"
  SetOutPath "$INSTDIR"
  File "bin\phazeai-ui.exe"
  File "bin\phazeai.exe"
  CreateShortcut "$DESKTOP\PhazeAI IDE.lnk" "$INSTDIR\phazeai-ui.exe"
  CreateDirectory "$SMPROGRAMS\PhazeAI IDE"
  CreateShortcut "$SMPROGRAMS\PhazeAI IDE\PhazeAI IDE.lnk" "$INSTDIR\phazeai-ui.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PhazeAI IDE" "DisplayName" "${APP_NAME} ${APP_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PhazeAI IDE" "UninstallString" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\phazeai-ui.exe"
  Delete "$INSTDIR\phazeai.exe"
  Delete "$INSTDIR\Uninstall.exe"
  Delete "$DESKTOP\PhazeAI IDE.lnk"
  Delete "$SMPROGRAMS\PhazeAI IDE\PhazeAI IDE.lnk"
  RMDir "$SMPROGRAMS\PhazeAI IDE"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PhazeAI IDE"
SectionEnd
