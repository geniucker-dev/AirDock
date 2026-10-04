; SPDX-License-Identifier: MPL-2.0
#ifndef SourceDir
  #error SourceDir must refer to an audited Windows package
#endif
#ifndef RepositoryURL
  #define RepositoryURL "https://github.com/geniucker-dev/AirDock"
#endif

[Setup]
AppId={{B088078A-7E1E-47D0-AD61-00CEC5C05AA7}
AppName=AirDock
AppVersion={#AppVersion}
AppPublisher=geniucker-dev
AppPublisherURL={#RepositoryURL}
AppSupportURL={#RepositoryURL}/issues
DefaultDirName={localappdata}\Programs\AirDock
DefaultGroupName=AirDock
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
UninstallDisplayIcon={app}\airdock.exe
CloseApplications=yes
RestartApplications=no
SetupLogging=yes
LicenseFile={#SourceDir}\LICENSE
SetupIconFile={#SourceDir}\airdock.ico
VersionInfoProductName=AirDock
VersionInfoDescription=AirDock Setup
VersionInfoProductVersion={#AppVersion}
VersionInfoVersion={#AppVersion}

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "{#SourceDir}\INSTALL_PAYLOAD.txt"; DestName: "next-payload.txt"; Flags: dontcopy

[Icons]
Name: "{group}\AirDock"; Filename: "{app}\airdock.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\AirDock"; Filename: "{app}\airdock.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\airdock.exe"; Description: "Open AirDock"; Flags: nowait postinstall skipifsilent

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
        Relative := Previous[I];
        if SafeRelativePath(Relative) and (Next.IndexOf(Relative) < 0) and
           FileExists(ExpandConstant('{app}\') + Relative) then
          if not DeleteFile(ExpandConstant('{app}\') + Relative) then
            RaiseException('Cannot remove an obsolete application file. Close AirDock and try again.');
      end;
    finally
      Previous.Free;
      Next.Free;
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Value, Exe: String;
begin
  if CurUninstallStep <> usUninstall then Exit;
  if RegQueryStringValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Run', 'AirDock', Value) then begin
    { Compare Unicode text directly. Generic script Trim/Copy conversions can use ANSI. }
    Exe := ExpandConstant('{app}\airdock.exe');
    if (CompareText(Value, '"' + Exe + '" --start-hidden') = 0) or
       (CompareText(Value, '"' + Exe + '"') = 0) or
       (CompareText(Value, Exe + ' --start-hidden') = 0) or
       (CompareText(Value, Exe) = 0) then begin
      if not RegDeleteValue(HKCU64, 'Software\Microsoft\Windows\CurrentVersion\Run', 'AirDock') then
        RaiseException('Cannot remove this installation autostart entry.');
      Log('Removed the autostart entry owned by this installation.');
    end else
      Log('Preserved an autostart entry belonging to another installation.');
  end;
end;
