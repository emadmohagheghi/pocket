; Pocket NSIS installer hooks.
;
; Install: the stock Tauri template force-creates a desktop shortcut for
; every silent install (the finish page that normally asks the user is
; skipped in silent mode), which made every in-app update drop a Pocket
; icon on the desktop. These hooks undo that — but only when this very
; install created the shortcut. One a user already had (finish-page
; checkbox on a manual install) is never touched.
;
; Uninstall: Pocket keeps its data next to the binary ($INSTDIR\PocketData)
; while the stock "delete app data" checkbox only clears the bundle-id
; dirs under %APPDATA% / %LOCALAPPDATA% — so notes and recordings survived
; uninstalls. The POSTUNINSTALL hook removes $INSTDIR\PocketData when the
; user asks for it.

Var PocketDesktopShortcutExisted

!macro NSIS_HOOK_PREINSTALL
  ; Runs before the installer (re)creates shortcuts: remember whether a
  ; desktop shortcut already exists.
  StrCpy $PocketDesktopShortcutExisted 0
  ${If} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
    StrCpy $PocketDesktopShortcutExisted 1
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Silent install = in-app update: remove the forced desktop shortcut,
  ; unless the user already had one before this install ran.
  ${If} ${Silent}
  ${AndIf} $PocketDesktopShortcutExisted = 0
    ${If} ${FileExists} "$DESKTOP\${PRODUCTNAME}.lnk"
      Delete "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Same gate as the stock "delete app data" block: only when the user
  ; ticked the checkbox, and never while an update is swapping builds.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Pocket keeps its data next to the binary ($INSTDIR\PocketData).
    ; The stock cleanup only knows the %APPDATA% / %LOCALAPPDATA% bundle
    ; dirs, so notes and recordings survived uninstalls. Remove the data
    ; folder, then the now-empty install folder itself.
    RmDir /r "$INSTDIR\PocketData"
    RmDir "$INSTDIR"
  ${EndIf}
!macroend
