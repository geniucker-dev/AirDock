#ifndef SourceDir
  #error SourceDir must refer to an audited Windows package
#endif

[Setup]
AppId={{5C60277C-4242-4C8E-A495-99D4E6D239B2}
AppName=AirPlay-Windows
AppVersion={#AppVersion}
AppPublisher=geniucker-dev
AppPublisherURL=https://github.com/geniucker-dev/AirPlay-Windows
AppSupportURL=https://github.com/geniucker-dev/AirPlay-Windows/issues
DefaultDirName={localappdata}\Programs\AirPlay-Windows
DefaultGroupName=AirPlay-Windows
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/fast
SolidCompression=yes
WizardStyle=modern
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\airplay-windows.exe
CloseApplications=yes
RestartApplications=no
SetupLogging=yes
LicenseFile={#SourceDir}\LICENSE

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\INSTALL_PAYLOAD.txt"; DestName: "next-payload.txt"; Flags: dontcopy

[Icons]
Name: "{group}\AirPlay-Windows"; Filename: "{app}\airplay-windows.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\AirPlay-Windows"; Filename: "{app}\airplay-windows.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\airplay-windows.exe"; Description: "Open AirPlay-Windows"; Flags: nowait postinstall skipifsilent

[Code]
function SafeRelativePath(const Path: String): Boolean;
begin
  if Path = '' then begin Result := False; Exit; end;
  Result := (Path[1] <> '\') and (Path[1] <> '/') and
    (Pos(':', Path) = 0) and (Pos('..', Path) = 0) and
    (Pos('*', Path) = 0) and (Pos('?', Path) = 0);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Previous, Next: TStringList;
  I: Integer;
  Relative: String;
begin
  if (CurStep = ssInstall) and FileExists(ExpandConstant('{app}\INSTALL_PAYLOAD.txt')) then begin
    ExtractTemporaryFile('next-payload.txt');
    Previous := TStringList.Create;
    Next := TStringList.Create;
    try
      Previous.LoadFromFile(ExpandConstant('{app}\INSTALL_PAYLOAD.txt'));
      Next.LoadFromFile(ExpandConstant('{tmp}\next-payload.txt'));
      for I := 0 to Previous.Count - 1 do begin
        Relative := Trim(Previous[I]);
        if SafeRelativePath(Relative) and (Next.IndexOf(Relative) < 0) and
           FileExists(ExpandConstant('{app}\') + Relative) then
          if not DeleteFile(ExpandConstant('{app}\') + Relative) then
            RaiseException('Cannot remove an obsolete application file. Close AirPlay-Windows and try again.');
      end;
    finally
      Previous.Free;
      Next.Free;
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value, Exe, Candidate: String;
  Quote: Integer;
begin
  if CurUninstallStep <> usUninstall then Exit;
  Log('Checking this installation autostart ownership.');
  if RegQueryStringValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Run', 'AirPlay-Windows', Value) then begin
    Value := Lowercase(Trim(Value));
    Exe := Lowercase(ExpandConstant('{app}\airplay-windows.exe'));
    Log('Expected autostart executable: ' + Exe);
    if Copy(Value, 1, 1) = '"' then begin
      Candidate := Copy(Value, 2, Length(Value));
      Quote := Pos('"', Candidate);
      if Quote > 0 then
        Log('Registered autostart executable: ' + Copy(Candidate, 1, Quote - 1));
    end;
    Log('Quoted executable prefix offset: ' + IntToStr(Pos('"' + Exe + '"', Value)));
    if (Value = Exe) or (Pos('"' + Exe + '"', Value) = 1) or (Pos(Exe + ' ', Value) = 1) then begin
      if not RegDeleteValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Run', 'AirPlay-Windows') then
        RaiseException('Cannot remove this installation autostart entry.');
      Log('Removed the 64-bit autostart entry owned by this installation.');
    end else
      Log('Preserved an autostart entry belonging to another installation.');
  end else
    Log('No autostart entry found.');
end;
