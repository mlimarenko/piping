!include "FileFunc.nsh"
!include "LogicLib.nsh"
!include "WinVer.nsh"

!insertmacro GetParameters
!insertmacro GetOptions

Var PipingServiceUrl

!macro PIPING_READ_SERVICE_URL_ARG
  StrCpy $PipingServiceUrl ""
  ${GetParameters} $R0

  ClearErrors
  ${GetOptions} $R0 "/SERVICE_URL=" $PipingServiceUrl
  ${If} ${Errors}
    ClearErrors
    ${GetOptions} $R0 "/PIPING_SERVICE_URL=" $PipingServiceUrl
  ${EndIf}
  ${If} ${Errors}
    ClearErrors
    ${GetOptions} $R0 "/SERVICEURL=" $PipingServiceUrl
  ${EndIf}
  ${If} ${Errors}
    ClearErrors
    StrCpy $PipingServiceUrl ""
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${AtLeastWin10}
    ${If} ${Silent}
      DetailPrint "Piping Desktop requires Windows 10 / Windows Server 2016 or newer."
    ${Else}
      MessageBox MB_ICONSTOP|MB_OK "Piping Desktop requires Windows 10 / Windows Server 2016 or newer.$\r$\n$\r$\nWindows Server 2012 and 2012 R2 are not supported: Microsoft Edge WebView2 Runtime is limited to version 109 on that OS, while Piping requires a supported WebView2 runtime.$\r$\n$\r$\nInstall Piping Desktop on Windows 10/11 or Windows Server 2016+."
    ${EndIf}
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro PIPING_READ_SERVICE_URL_ARG
  ${If} $PipingServiceUrl != ""
    ClearErrors
    FileOpen $R1 "$INSTDIR\desktop-service-url.txt" w
    ${If} ${Errors}
      DetailPrint "Failed to write Piping service URL seed."
    ${Else}
      FileWrite $R1 "$PipingServiceUrl$\r$\n"
      FileClose $R1
      DetailPrint "Configured Piping service URL from installer parameter."
    ${EndIf}
  ${EndIf}
!macroend
