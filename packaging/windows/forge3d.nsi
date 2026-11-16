; forge3d by ZuziaDev - Windows installer (NSIS). Build: packaging/windows/build.sh
Unicode true
!include "MUI2.nsh"
!include "WordFunc.nsh"

!ifndef VERSION
  !define VERSION "0.1.1"
!endif
!ifndef EXE
  !define EXE "forge3d.exe"
!endif
!ifndef OUTFILE
  !define OUTFILE "forge3d-setup.exe"
!endif

Name "forge3d ${VERSION}"
BrandingText "forge3d by ZuziaDev"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "forge3d"
VIAddVersionKey "CompanyName" "ZuziaDev"
VIAddVersionKey "FileDescription" "forge3d installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (C) ZuziaDev"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\forge3d"
InstallDirRegKey HKCU "Software\forge3d" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\forge3d"

!define MUI_ICON "assets\icon.ico"
!define MUI_UNICON "assets\icon.ico"
!define MUI_ABORTWARNING
!define MUI_HEADERIMAGE
!define MUI_HEADERIMAGE_BITMAP "assets\installer-header.bmp"
!define MUI_HEADERIMAGE_RIGHT
!define MUI_WELCOMEFINISHPAGE_BITMAP "assets\installer-welcome.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "assets\installer-welcome.bmp"
!define MUI_WELCOMEPAGE_TITLE "forge3d ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "forge3d by ZuziaDev: a 3D modeler with a built-in MCP server, so people and AI assistants edit the same scene.$\r$\n$\r$\nThis installs forge3d for the current user. No administrator rights needed."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\forge3d.exe"
!define MUI_FINISHPAGE_RUN_PARAMETERS "gui"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\claude_desktop_config.example.json"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Show Claude Desktop config example"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_WELCOME
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Turkish"

Section "forge3d" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "/oname=forge3d.exe" "${EXE}"
  File "README.md"
  SetOutPath "$INSTDIR\examples"
  File "examples\*.json"
  SetOutPath "$INSTDIR"

  CreateDirectory "$DOCUMENTS\forge3d"

  ; Claude Desktop config example with real, JSON-escaped paths.
  FileOpen $0 "$INSTDIR\claude_desktop_config.example.json" w
  ${WordReplace} "$INSTDIR\forge3d.exe" "\" "\\" "+" $1
  ${WordReplace} "$DOCUMENTS\forge3d" "\" "\\" "+" $2
  FileWrite $0 '{$\r$\n  "mcpServers": {$\r$\n    "forge3d": {$\r$\n'
  FileWrite $0 '      "command": "$1",$\r$\n'
  FileWrite $0 '      "args": ["mcp", "--workspace", "$2"]$\r$\n'
  FileWrite $0 '    }$\r$\n  }$\r$\n}$\r$\n'
  FileClose $0

  CreateDirectory "$SMPROGRAMS\forge3d"
  CreateShortcut "$SMPROGRAMS\forge3d\forge3d.lnk" "$INSTDIR\forge3d.exe" "gui --workspace $\"$DOCUMENTS\forge3d$\"" "$INSTDIR\forge3d.exe" 0
  CreateShortcut "$SMPROGRAMS\forge3d\Uninstall forge3d.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\forge3d.lnk" "$INSTDIR\forge3d.exe" "gui --workspace $\"$DOCUMENTS\forge3d$\"" "$INSTDIR\forge3d.exe" 0

  WriteRegStr HKCU "Software\forge3d" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "forge3d"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/ZuziaDev/forge3d"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "ZuziaDev"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\forge3d.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\forge3d.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\claude_desktop_config.example.json"
  Delete "$INSTDIR\examples\*.json"
  RMDir "$INSTDIR\examples"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\forge3d\*.lnk"
  RMDir "$SMPROGRAMS\forge3d"
  Delete "$DESKTOP\forge3d.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\forge3d"
SectionEnd
