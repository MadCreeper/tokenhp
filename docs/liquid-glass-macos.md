# Liquid Glass popover (macOS only)

Status: **implemented**. Classic/HP theme, macOS 26+ (Tahoe/26.x). Everything
here is macOS-specific — Linux/Windows and the other themes keep the solid
opaque popover and are unaffected.

The popover renders on Apple's native **Liquid Glass** material
(`NSGlassEffectView`), behaving like a desktop widget: **near-solid when
focused, clear liquid glass when unfocused**, and reactive to the system's
Liquid Glass appearance setting.

This doc records how it works and — more usefully — the pitfalls and dead ends,
because almost none of this is documented by Apple and several "obvious"
approaches silently fail or crash.

---

## Architecture

A native glass view sits **behind a transparent WKWebView**; the web UI (the
HP-bar panel) renders on top.

```
NSWindow (transparent, macOSPrivateApi)
 └─ contentView
     ├─ NSGlassEffectView        ← native backdrop, inserted at the bottom
     └─ WKWebView (transparent)  ← our panel; glass shows through where CSS is transparent
```

Key pieces, all in `src-tauri/src/lib.rs`:

- `build_popover()` — builds a `transparent(true)` window, calls
  `apply_liquid_glass()`, then clears the webview background.
- `apply_liquid_glass()` — inserts the `NSGlassEffectView` (via runtime `objc2`,
  no SDK dependency), sets the material variant, and installs the key-appearance
  override. Returns `false` on pre-26 macOS so the caller falls back to
  `window-vibrancy`'s `Popover` material.
- `force_key_window_appearance()` — the critical fix (see below).

Frontend: `src/styles.css` gates all glass styling on `html.mac` (stamped by
`src/main.ts`; mock/showcase pages skip it). The focused/unfocused split is a
`blurred` class toggled from Rust.

Test harness: **`cargo run --example glass_check`**
(`src-tauri/examples/glass_check.rs`) — opens a grid of windows comparing every
vibrancy material, light/dark, and glass style/variant, with live tint sliders.
This is the tool that cracked every question below; keep it.

---

## The four things that had to be right

### 1. Transparent webview — clear `underPageBackgroundColor`

Setting the Tauri window `transparent: true` is **not enough**. wry disables
`drawsBackground` but leaves WKWebView's `underPageBackgroundColor` at its
opaque macOS-12+ default, which paints **white behind the transparent page** —
so the window composites transparent (rounded corners show the desktop) but the
interior is a flat white panel.

Fix: in `with_webview`, `setOpaque:NO` **and**
`setUnderPageBackgroundColor: NSColor.clearColor`.

Requires: tauri `macos-private-api` feature + `"macOSPrivateApi": true` in
`tauri.conf.json` + `.transparent(true)` on the window.

### 2. Clear-while-unfocused — override `hasKeyAppearance` (THE hard one)

`NSGlassEffectView` renders clear **only while its window has _key_
appearance**. The moment the window isn't key it clouds to a frosted grey — and
"not key" is exactly the unfocused state where we *want* the clear widget look.
Desktop widgets look clear when unfocused because their host window reports
active/key appearance permanently.

Fix (`force_key_window_appearance`): override `hasKeyAppearance` → `YES` **in
place** on the popover window's class:

```rust
let sel = objc2::ffi::sel_registerName(c"hasKeyAppearance".as_ptr());
objc2::ffi::class_addMethod(cls, sel, Some(yes_imp), c"c@:".as_ptr());
```

- `hasKeyAppearance` is **public** `NSWindow` API — we only override a getter.
- `class_addMethod` modifies the existing class in place. Do **not** use
  `object_setClass` to reparent onto a `ClassBuilder` subclass — that
  **stack-overflows** the tao window (broken super-dispatch chain).
- The class override is process-wide for tao windows; harmless for the app's
  other (non-glass) windows.
- The Tauri `Focused` **event** is independent of `hasKeyAppearance`, so the
  CSS focus-state toggle keeps working.

### 3. Clear material — variant, not the default style

The default `.regular` glass style is inherently frosted/milky; even with the
key-appearance override it stays cloudy. A **clear** material is needed on top:

- `set_variant: 4` ("Widgets" material — what desktop widgets use). **Private**
  API, guarded by `respondsToSelector`, falls back to default. Currently used.
- `setStyle: 1` ("clear") — **public** API, also stays clear with the override.
  A drop-in alternative if you want 100% public API.

Do **not** force `setStyle:` if you want the glass to keep tracking **System
Settings → Appearance → Liquid Glass (Clear/Tinted)** — the default style
follows that slider; the variant is orthogonal to it.

### 4. CSS: near-solid focused, bare glass unfocused

`src/styles.css`, gated on `html.mac body.theme-classic`:

- **Focused**: `background: rgba(245,245,248,0.94)` — near solid, full colour,
  dark text. Not lower: clear glass barely blurs, so under ~0.9 crisp dark
  content bleeds through and reads translucent over dark windows.
- **Unfocused** (`html.mac.blurred`): `background: transparent` — **no CSS
  film**. The native glass alone carries the look, so it's exactly as
  clear/frosted as the system setting dictates. Any constant tint here becomes
  an opacity floor that keeps the clear end from ever reaching the system's.
  White text + shadow for legibility; chrome (tabs/tracks/buttons) made
  translucent so it doesn't add whiteness; `filter: grayscale(1)` for the
  monochrome glass look.
- A `transition` animates the focused↔unfocused morph.
- `@media (prefers-reduced-transparency: reduce)` → solid, matching native glass.

The `blurred` class is toggled **from Rust** in the `WindowEvent::Focused`
handler via `win.eval(...)` — see pitfalls for why not from JS.

---

## Dead ends (do not retry these)

| Attempt | Result |
|---|---|
| `transparent: true` alone | White webview interior (underPageBackgroundColor). |
| Toggle `blurred` from JS `focus`/`blur` events | Tracks in-page first-responder churn, not window key state — reads inverted. |
| Toggle `blurred` from `getCurrentWindow().onFocusChanged` | Silently never fired in the WKWebView. |
| Swizzle glass `_windowChangedKeyState` to no-op | Frost lives deeper (backdrop layer); no effect. |
| `NSWindow._setHasActiveAppearance: YES` (re-assert) | Glass reads *key* appearance, not *active*; no effect. |
| `[window acquireKeyAppearance]` re-assert (poll) | Works momentarily → **visible blink** as AppKit reverts each tick. |
| `object_setClass` onto a `ClassBuilder` subclass | **Stack overflow** (tao window super-chain). |
| Default `.regular` glass + override | Still cloudy — needs a clear variant/style. |
| CSS film (e.g. `rgba(...,0.10)`) in unfocused state | Opacity floor; can't reach the system's clear end. |

Also: `NSApp deactivate` does **not** emit `Focused(false)` — to test the
unfocused state, activate another app (`open -a TextEdit`).

---

## Testing notes (learned the hard way)

- **Judge clear-vs-cloudy over a DARK / high-contrast backdrop.** Over a bright
  wallpaper or light windows, frosted-grey and clear glass look nearly
  identical — this masked two false "it works" conclusions.
- **A window's glass blurs whatever is _behind the window_** (other windows, or
  the wallpaper where none). Desktop widgets get privileged compositing that
  blurs the wallpaper even with windows in front, so over a light/white window
  our popover is milkier than a widget over warm wallpaper. Minor next to the
  key-appearance issue, but it means an exact widget match isn't always possible.
- Debug hook: `HPBAR_DEBUG_SHOW=1 cargo run` shows the popover pinned at launch
  and force-activates it (Accessory apps need `activateIgnoringOtherApps:`).
  Screenshot with `screencapture -x`.
- **`pkill -x hpbar` also kills a running `tauri dev` instance** (same binary
  name), and that takes Vite down with it. Free port 1420 before restarting.
- Cargo's mtime cache sometimes no-ops a rebuild after an edit; `touch
  src/lib.rs` to force it.

---

## Version / risk notes

- Requires macOS 26+ for `NSGlassEffectView`; older macOS falls back to
  `window-vibrancy` (`Popover` material, `state = Active` so it never greys).
- **Private API churn is real.** `setVariant:` (from an earlier macOS 26.0
  reference) was already renamed to `set_variant:` by macOS 27. Introspect with
  `swift -e` + `class_copyMethodList` when a selector stops working. The only
  private call in the shipping path is `set_variant: 4`; it's guarded and
  degrades to the default material. `hasKeyAppearance` and `setStyle:` are
  public.
- Related: `macos-tray-icon-rendering`, `macos-tahoe-tray-menu` (other
  macOS-specific rendering gotchas in this app).
