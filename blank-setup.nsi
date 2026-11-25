Unicode true
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Blank Launcher"
Name "Blank Launcher"
OutFile "Blank-Setup.exe"
ShowInstDetails show

Section
  SetOutPath "$INSTDIR"
  File "target\release\riven.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortCut "$SMPROGRAMS\Blank Launcher.lnk" "$INSTDIR\riven.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlankLauncher" "DisplayName" "Blank Launcher"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlankLauncher" "UninstallString" '"$INSTDIR\uninstall.exe"'
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\Blank Launcher.lnk"
  Delete "$INSTDIR\riven.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\BlankLauncher"
SectionEnd

Function .onInstSuccess
  IfSilent done
  Exec '"$INSTDIR\riven.exe"'
done:
FunctionEnd
