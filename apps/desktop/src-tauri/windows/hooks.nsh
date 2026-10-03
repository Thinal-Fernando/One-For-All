; Extra installer steps. ofa.exe is installed next to OFA.exe; these run it
; to add or remove OFA's Claude Code hooks and its PATH entry.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Connecting OFA to Claude Code..."
  nsExec::ExecToLog '"$INSTDIR\ofa.exe" setup --path'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; An update reinstalls over the old copy; keep the hooks and PATH then.
  ${If} $UpdateMode <> 1
    DetailPrint "Disconnecting OFA from Claude Code..."
    nsExec::ExecToLog '"$INSTDIR\ofa.exe" setup --uninstall --path'
    Pop $0
  ${EndIf}
!macroend
