# Clipboard Platform Todo

## Goal

Implement raw clipboard support across platforms without pretending browser clipboard
access is synchronous. macOS has the first raw clipboard implementation; finish the
API shape, implement wasm next, then fill in Windows and Linux.

## Current State

- `RawClipboardItem` / `RawClipboardEntry` are implemented in `src/platform.rs`.
- `Platform::read_raw_from_clipboard` has a default lossy conversion from
  `ClipboardItem`.
- macOS `Pasteboard::read_raw` reads native pasteboard types and bytes.
- Test helpers can store/read raw clipboard entries.
- wasm sync methods still return `None` / no-op because browser clipboard access
  is async.
- wasm async methods use the browser Clipboard API for text, raw MIME entries,
  and supported image MIME entries.
- wasm async methods start the browser clipboard promise synchronously before
  dispatching the await work, so paste/copy can still run under browser
  user-activation rules.
- wasm target checking now uses the existing non-`font-kit` font-matching
  fallback, because `font-kit` is only declared for native targets.
- Windows raw reads enumerate clipboard formats, name standard/custom formats,
  and copy lockable `HGLOBAL` payloads.
- Wayland raw reads use the current data-offer MIME list and read each offered
  MIME payload.
- X11 raw reads query `TARGETS`, request each payload target, and expose atom
  names as raw entry formats.

## API Shape

- Status: implemented in the pending worktree.
- Keep existing sync APIs for desktop compatibility:
  - `read_from_clipboard(&self) -> Option<ClipboardItem>`
  - `write_to_clipboard(&self, item: ClipboardItem)`
  - `read_raw_from_clipboard(&self) -> Option<RawClipboardItem>`
- Add async/fallible APIs for platforms that require them:
  - `read_clipboard(&self) -> Task<Result<Option<ClipboardItem>>>`
  - `read_raw_clipboard(&self) -> Task<Result<Option<RawClipboardItem>>>`
  - `write_clipboard(&self, item: ClipboardItem) -> Task<Result<()>>`
- Desktop defaults should wrap existing sync implementations.
- wasm should override with browser Clipboard API calls.

## Wasm First

- Status: implemented in the pending worktree.
- Add required `web-sys` features:
  - `Clipboard`
  - `ClipboardItem`
  - `Blob`
  - `BlobPropertyBag`
  - any array-buffer helpers needed by the implementation
- Add `src/platform/web/clipboard.rs`.
- Implement raw read:
  - use `navigator.clipboard.read()`
  - iterate each `ClipboardItem.types`
  - call `getType(mime)`
  - read each `Blob` as bytes
  - return `RawClipboardEntry { format: mime, bytes }`
- Implement high-level read:
  - prefer raw read and convert known formats into `ClipboardEntry`
  - fallback to `readText()` when raw read is unavailable
- Implement write:
  - use `writeText()` for plain text-only clipboard items
  - use `ClipboardItem` plus `Blob` for image/raw MIME entries
  - return clear errors for missing API, permission denial, insecure context, or
    lack of user activation

## Other Platforms

- Windows:
  - Status: implemented.
  - enumerate formats with `EnumClipboardFormats`
  - name formats using known `CF_*` labels or `GetClipboardFormatNameW`
  - copy `HGLOBAL` bytes into `RawClipboardEntry`
- Wayland:
  - Status: implemented.
  - use existing `DataOffer` MIME-type tracking
  - read each offered MIME type into raw entries
  - preserve current high-level text/image behavior
- X11:
  - Status: implemented.
  - query `TARGETS`
  - request each target
  - convert atoms to names
  - preserve current high-level text/image behavior
- Visual/test platforms:
  - keep raw clipboard storage mirrored with high-level clipboard storage
  - Status: implemented.
  - add tests for raw storage and fallback conversion

## Tests

- Unit-test `RawClipboardItem::from_clipboard_item`: implemented.
- Unit-test test-platform raw write/read behavior: implemented.
- wasm compile check for `wasm32-unknown-unknown`: passing in the pending
  worktree.
- Add platform helper tests where practical:
  - mac pasteboard raw entries: implemented.
  - Windows format-name mapping: implemented.
  - Wayland MIME conversion: implemented.
  - X11 `TARGETS` parsing: implemented.
  - X11 atom-name conversion: still needs a live X11 integration test or a
    mockable atom-name layer.

## Notes

- The browser Clipboard API is async and permission-gated. Do not implement wasm
  by returning a stale sync cache from `read_from_clipboard` and calling that
  complete.
- `navigator.clipboard.read()` and `write()` can fail when the document lacks
  focus, permission, secure context, or user activation. These should surface as
  errors from the async API.
- The sync API can remain best-effort for existing call sites, but new clipboard
  integrations should use the async/fallible methods.
