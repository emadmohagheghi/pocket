// Window dragging for the frameless windows (main + quick-capture).
//
// Vendored from Tauri's built-in drag-region script
// (tauri/src/window/scripts/drag.js in the Tauri repository) and heavily
// adapted. Stock Tauri only drags from elements that opt in with
// data-tauri-drag-region — after the redesign that left only thin empty
// strips of the window draggable. This version flips the model: EVERYTHING
// drags the window by default, and interactive elements (buttons, fields,
// links, floating menus) opt out on their own, so layouts need no markers.
//
// It also wins the race against Tauri's stock copy of the script (injected
// as an init script into every webview before any page script): that copy
// has no scrollbar guard and freezes input focus, so this module registers
// its mousedown listener in the CAPTURE phase and blocks the stock copy
// whenever it handles a press itself.
//
// Original license headers preserved:
//
// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

declare global {
  interface Window {
    __TAURI_INTERNALS__: {
      invoke(cmd: string, args?: unknown): Promise<unknown>;
      metadata?: {
        currentWindow?: { label?: string };
      };
    };
  }
}

;(function () {
  // The image viewer must never move: it is a fixed fullscreen overlay
  // (opened maximized, closed only via its X button). This script drags the
  // window on almost every press by default, so it opts out entirely.
  try {
    if (window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label === 'image-viewer') return
  } catch {
    /* no metadata: behave as any other window */
  }

  const TAURI_DRAG_REGION_ATTR = 'data-tauri-drag-region'
  const INTERACTIVE_TAGS = new Set([
    'A',
    'BUTTON',
    'INPUT',
    'SELECT',
    'TEXTAREA',
    'LABEL',
    'SUMMARY'
  ])
  const INTERACTIVE_ROLES = new Set([
    'button',
    'link',
    'menuitem',
    'menuitemcheckbox',
    'menuitemradio',
    'menu',
    'tab',
    'checkbox',
    'radio',
    'switch',
    'option',
    'combobox',
    'listbox',
    'textbox',
    'searchbox',
    'slider',
    'scrollbar'
  ])
  // Radix portals floating panels (menus, selects, popovers) into a fixed
  // wrapper outside the app tree. Their items are interactive and would
  // block dragging anyway; this check also covers panel padding.
  const FLOATING_PANEL_SELECTOR = '[data-radix-popper-content-wrapper]'

  function isInteractive(el: HTMLElement): boolean {
    return (
      INTERACTIVE_TAGS.has(el.tagName) ||
      (el.hasAttribute('contenteditable') &&
        el.getAttribute('contenteditable') !== 'false') ||
      INTERACTIVE_ROLES.has(el.getAttribute('role') || '')
    )
  }

  // Walk the composed path from the mousedown target to the root; the first
  // deciding element wins:
  //
  //   data-tauri-drag-region="false"  → never drag (sticky for the subtree)
  //   Radix floating panel            → never drag
  //   data-tauri-drag-region (truthy) → explicit drag region; also opts in
  //                                     to double-click maximize
  //   interactive element or ancestor → buttons, fields, links… never drag
  //   anything else                   → drags the window
  function decideDrag(composedPath: EventTarget[]): {
    drag: boolean
    explicit: boolean
  } {
    for (const target of composedPath) {
      if (!(target instanceof HTMLElement)) continue
      const el = target
      const attr = el.getAttribute(TAURI_DRAG_REGION_ATTR)
      if (attr === 'false') return { drag: false, explicit: false }
      if (el.matches(FLOATING_PANEL_SELECTOR)) return { drag: false, explicit: false }
      if (attr !== null) return { drag: true, explicit: true }
      if (isInteractive(el)) return { drag: false, explicit: false }
    }
    // Default: every non-interactive press drags the window.
    return { drag: true, explicit: false }
  }

  // Native scrollbars are not elements: a mousedown on a scrollbar thumb or
  // track targets the scroll container itself (a plain div), so without this
  // check grabbing the thumb would start a window drag instead of scrolling.
  function isOnScrollbar(e: MouseEvent, composedPath: EventTarget[]): boolean {
    for (const target of composedPath) {
      if (!(target instanceof HTMLElement)) continue
      const el = target
      const rect = el.getBoundingClientRect()
      if (rect.width <= 0 || rect.height <= 0) continue
      // clientWidth/clientHeight exclude the scrollbar, so any leftover
      // width/height on a scrollable element is (at least) the scrollbar.
      const vBar = rect.width - el.clientWidth
      if (vBar > 0 && el.scrollHeight > el.clientHeight) {
        if (e.clientX >= rect.right - vBar) return true
        // In RTL the vertical scrollbar sits on the left edge.
        if (getComputedStyle(el).direction === 'rtl' && e.clientX <= rect.left + vBar) {
          return true
        }
      }
      const hBar = rect.height - el.clientHeight
      if (hBar > 0 && el.scrollWidth > el.clientWidth && e.clientY >= rect.bottom - hBar) {
        return true
      }
    }
    return false
  }

  // Upstream fills this in at build time; detect at runtime instead.
  const osName: string = /mac/i.test(navigator.userAgent) ? 'macos' : 'windows'

  // initial mousedown position for macOS
  let initialX = 0
  let initialY = 0

  // Capture phase: must run BEFORE Tauri's stock drag script (a document-level
  // bubble listener injected as an init script). We stopImmediatePropagation()
  // whenever the stock copy must not act — it has no blur handling and no
  // scrollbar guard, so letting it run reintroduces both bugs.
  document.addEventListener(
    'mousedown',
    (e) => {
      const path = e.composedPath()
      // Native scrollbar interaction: block the stock drag script and let the
      // browser scroll natively.
      if (isOnScrollbar(e, path)) {
        e.stopImmediatePropagation()
        return
      }
      const { drag, explicit } = decideDrag(path)
      if (
        // was left mouse button
        e.button === 0 &&
        // and was normal click to drag or double click to maximize
        (e.detail === 1 || e.detail === 2) &&
        // and is draggable
        drag
      ) {
        // macOS maximization happens on `mouseup`,
        // so we save needed state and early return
        if (osName === 'macos' && e.detail === 2) {
          initialX = e.clientX
          initialY = e.clientY
          return
        }

        // Clicking window chrome must release focus explicitly: an input
        // would otherwise keep its caret (and focus ring) when clicking
        // elsewhere. Presses on interactive elements never reach here.
        const active = document.activeElement
        if (active instanceof HTMLElement && active !== e.target) active.blur()

        // Explicit regions keep the stock script's text-cursor prevention;
        // default regions skip it so plain clicks still move focus (e.g. a
        // feed row must receive focus to keep its keyboard flow alive).
        if (explicit) e.preventDefault()

        // Also blocks Tauri's stock drag script from handling the same event.
        e.stopImmediatePropagation()

        // Explicit regions maximize on double-click; default regions just
        // drag — the windows are fixed-size, so maximizing from anywhere
        // would be surprising.
        const cmd =
          explicit && e.detail === 2 ? 'internal_toggle_maximize' : 'start_dragging'
        const internals = window.__TAURI_INTERNALS__
        if (internals) {
          void internals.invoke('plugin:window|' + cmd).catch(() => {})
        }
      }
    },
    true
  )

  // on macOS we maximize on mouseup instead, to match the system behavior where maximization can be canceled
  // if the mouse moves outside the data-tauri-drag-region
  if (osName === 'macos') {
    document.addEventListener('mouseup', (e) => {
      if (
        // was left mouse button
        e.button === 0 &&
        // and was double click
        e.detail === 2 &&
        // and the cursor hasn't moved from initial mousedown
        e.clientX === initialX &&
        e.clientY === initialY &&
        // and not on a scrollbar
        !isOnScrollbar(e, e.composedPath()) &&
        // and the press landed in an explicit drag region
        decideDrag(e.composedPath()).explicit
      ) {
        const internals = window.__TAURI_INTERNALS__
        if (internals) {
          void internals.invoke('plugin:window|internal_toggle_maximize').catch(() => {})
        }
      }
    })
  }
})()

export {}
