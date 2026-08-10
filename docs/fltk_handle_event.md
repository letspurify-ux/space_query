# FLTK Event Handling

> Implementation: `src/ui/table_browse.rs`, `src/ui/query_tabs.rs`,
> `src/ui/tab_strip.rs`, `src/ui/result_table.rs`, `src/ui/object_browser.rs`,
> `src/ui/main_window.rs`, `src/ui/theme.rs`
>
> Upstream: `fltk` 1.5 (`cfltk/include/cfltk/cfl_widget.hpp`,
> `fltk/src/Fl.cxx`, `fltk/src/Fl_Group.cxx`)

The recurring question when adding `widget.handle(...)` is: *how do I make sure
I only react to events that are actually mine?* Most of the answer is knowing
what FLTK already routed for you, and filtering only the rest.

## 1. What FLTK routes for you

`Fl::handle()` picks the recipient per event class before any widget closure
runs. These are the rules that matter (`Fl.cxx`, `Fl_Group.cxx`):

| Event | Initial recipient | Propagation |
| --- | --- | --- |
| `Push` | Deepest visible child containing the pointer | Group walks children in **reverse add order**; first child returning `true` wins and becomes `Fl::pushed()` |
| `Drag`, `Released` | `Fl::pushed()` only | No search — goes straight to the Push winner, even outside its box |
| `Enter`, `Move` | Child under the pointer | Child must return `true` from `Enter` to stay `belowmouse()`; otherwise the group claims it and `Move`/`Leave` stop arriving |
| `MouseWheel` | Window, then `belowmouse()` chain | Bubbles to parents |
| `KeyDown`, `KeyUp` | `Fl::focus()` | Bubbles **focus → parent → … → window** while handlers return `false` |
| `Shortcut` | `belowmouse()` chain, then **every child of every group** | Effectively broadcast (see §5) |
| `Focus`, `Unfocus` | Widget gaining/losing focus | Return `true` from `Focus` to accept it |
| `DndEnter`/`DndDrag`/`DndRelease`/`Paste` | Child under pointer | Must return `true` from `DndEnter` to receive the later ones |

Consequences:

- Mouse position filtering is usually **not** needed for `Push`/`Move` on a leaf
  widget — FLTK already tested `Fl::event_inside(o)`.
- Grab is opt-in: if you want `Drag`/`Released`, you must return `true` from
  `Push`. `src/ui/main_window.rs:5926` (splitter drag) relies on exactly this.
- Keyboard filtering **is** always needed, because parents see every key their
  children declined.

## 2. fltk-rs `handle()` semantics (the part that surprises people)

`WidgetBase::handle` installs a closure into a cfltk-derived subclass. Its
dispatch is in `cfl_widget.hpp:70`:

```cpp
int handle(int event) override {
    if (super_handle_first) {          // default: true
        int ret = T::handle(event);    // built-in handler runs FIRST
        if (inner_handler) {
            int local = inner_handler(this, event, ev_data_);
            return ret | local;        // BOTH always run, results OR'd
        }
        return ret;
    } else {
        if (inner_handler && inner_handler(this, event, ev_data_))
            return 1;                  // your closure pre-empts the built-in
        return T::handle(event);
    }
}
```

Four practical rules follow.

### 2.1 `super_handle_first` defaults to `true`

By default the widget's native behavior runs **before** your closure and always
runs. Returning `true` does **not** suppress it — it only marks the event as
handled toward the parent.

To pre-empt native behavior, call `super_handle_first(false)` before installing
the handler:

```rust
// src/ui/query_tabs.rs:306
tabs.super_handle_first(false);   // consume header wheel before Fl_Tabs
tabs.handle(move |tabs, ev| { ... });
```

Choose deliberately, and record the choice:

- **Keep `true`** when the native handler does the real work and you only track
  state. `src/ui/result_table.rs:2212` keeps it so `Fl_Table`'s native `Drag`
  drives selection and auto-scroll.
- **Set `false`** when you must act before the native handler mutates state:
  `Fl_Tabs` offset (`src/ui/query_tabs.rs:306`), `Fl_Browser` release-selection
  (`src/ui/intellisense.rs:3149`), `Fl_Choice` popup (`src/ui/object_browser.rs:962`).

It can also be flipped mid-gesture. `src/ui/tab_strip.rs:751` switches to
native-first on header `Push` so FLTK still performs tab selection, then
restores `false` on `Released`/`Unfocus`/`Deactivate`/`Hide` — note that the
reset must be on *every* terminating event, not just `Released`.

### 2.2 `handle()` replaces, it does not chain

Each call drops the previously boxed closure. A second `handle()` on the same
widget silently disables the first. `theme::install_button_hover` installs one
(`src/ui/theme.rs:242`), so a later `handle()` on a themed button kills the
hover feedback unless the new closure calls the hover update itself.

### 2.3 The widget must be *derived*

`handle()` starts with `assert!(self.is_derived)`. Widgets obtained through
`from_widget_ptr`, `unsafe from_dyn_widget`, or `Fl_Widget*` round-trips (e.g.
`app::belowmouse()`, `app::focus()`) are not derived and will panic.

### 2.4 Panics are swallowed

The shim wraps your closure in `catch_unwind` and turns a panic into `0`. A
panicking handler looks like a dead handler, not a crash — hence the
`unwrap_or_else(|poisoned| poisoned.into_inner())` lock idiom used throughout
`src/ui`.

## 3. Filtering recipes

### Keyboard — gate on focus

```rust
input.handle(move |input, ev| match ev {
    Event::KeyDown if input.has_focus() => { /* handle */ true }
    _ => false,
});
```

`src/ui/table_browse.rs` re-checks `input.has_focus()` at every branch
(`:492`, `:614`, `:629`) rather than once at the top, because a branch may run
after focus moved.

Comparing against the global focus owner:

```rust
if !app::focus().map(|f| f.is_same(w)).unwrap_or(false) { return false; }
```

### Mouse — gate on geometry

Needed for window/group-level handlers and for sub-regions of a custom-drawn
widget (a header strip, a splitter hot zone):

```rust
if !app::event_inside_widget(w) { return false; }        // widget box
if !app::event_inside(x, y, ww, hh) { return false; }    // arbitrary rect
```

`src/ui/main_window.rs:1649` does the same comparison inline because it needs
the hover state for both the inside and outside cases.

### Identity — gate on the widget itself

When one closure serves several widgets, or when it captures a different widget:

```rust
if !w.is_same(&target) { return false; }
if w.as_widget_ptr() != target.as_widget_ptr() { return false; }  // src/ui/tab_strip.rs:566
```

### Sub-region — gate on a pure function

Hit tests belong in a testable free function, not in the closure.
`src/ui/tab_strip.rs` exposes `point_is_in_tab_header`,
`point_is_in_pulldown_button`, and `should_consume_mouse_wheel`, all covered by
unit tests, so the closure stays a thin dispatcher.

## 4. Return-value discipline

`true` = consumed, stop bubbling. `false` = not mine, pass on.

- **Never** return `true` for an event you did not act on. A parent returning
  `true` for `KeyDown` blocks every child shortcut in the subtree.
- Returning `false` while still doing work is a legitimate and common pattern —
  observe-only handlers do exactly this: hover repaint (`src/ui/theme.rs:244`),
  gesture recording (`src/ui/intellisense.rs:3150`), popup hiding on window
  `Resize`/`Hide` (`src/ui/main_window.rs:11872`).
- With `super_handle_first = true`, `false` is *still* correct for observe-only
  handlers: the native result passes through unchanged via `ret | 0`.

## 5. Gotchas that have actually cost time here

### `Shortcut` is broadcast

`Fl_Group::handle(FL_SHORTCUT)` (`Fl_Group.cxx:175`) sends the event to **all**
children — those under the pointer first, then all the rest — regardless of
focus. Any `Event::Shortcut` arm must gate on focus itself:

```rust
// src/ui/table_browse.rs:613
Event::Shortcut => {
    if !input.has_focus() { return false; }
    ...
}
```

### An unconsumed navigation key scrolls a pane you never touched

`Fl_Scrollbar::handle()` (`Fl_Scrollbar.cxx:80`) folds `FL_SHORTCUT` into the same
branch as `FL_KEYBOARD`, and that branch acts on `Up`, `Down`, `Page_Up`,
`Page_Down`, `Home`, `End` (vertical) or `Left`/`Right` (horizontal) **without
checking focus at all**. Only `Page_Up`/`Page_Down` bail out early when there is
nothing to scroll; `Home`/`End` return 1 unconditionally.

So any navigation key the focused widget declines takes this route:

1. `Fl.cxx` finishes the `FL_KEYBOARD` walk (focus → parents) with nothing handled.
2. It re-enters as `FL_SHORTCUT`, starting at **`belowmouse()`** and walking up.
3. Each group on that path broadcasts to all of its children (§5).
4. The first scrollbar reached scrolls — in a pane the keystroke was never for.

The pointer is what decides the victim, which is why this reproduces from one
entry path and not another. `ResultTableWidget` hit it after a double-click in
the object browser opened a table: the mouse was still resting over the tree, so
the tree's scrollbar answered.

`Fl_Table` makes it easy to trip. Its `FL_KEYBOARD` branch routes every
navigation key through `move_cursor()`, which returns 0 when the target cell
equals the current one (`Fl_Table.cxx`) — so the grid silently declines on its
first/last row and first/last column, exactly where a user presses hardest.

The sink cannot be fixed: fltk-rs `handle()` has no way to express "skip the
native handler but do not consume" (§2), and `Fl_Tree`'s internal scrollbars are
not derived widgets, so they cannot take a closure (§2.3). **The focused widget
must consume the whole key set it navigates with, whether or not anything
moved.** `ResultTableWidget::grid_owns_navigation_key` is that list, applied once
at the end of the `KeyDown` arm and gated on the widget owning focus.

### Enter steals focus

In `FL_SHORTCUT`, after all children decline, `Fl_Group::handle` runs
`navigation(FL_Down)` for `Enter`/`KP_Enter` (`Fl_Group.cxx:186`) — pressing
Enter moves focus to the next widget. A filter input that submits on Enter must
re-assert focus, and must do it on `KeyUp` as well as `KeyDown` because
navigation can fire between the two:

```rust
// src/ui/table_browse.rs:625
Event::KeyUp => {
    if matches!(key, Key::Enter | Key::KPEnter) {
        if !input.has_focus() { return false; }
        Self::retain_input_focus(input);   // take_focus() now + again at timeout 0.0
        return true;
    }
    ...
}
```

`retain_input_focus` (`src/ui/table_browse.rs:800`) takes focus twice — once
immediately and once from a zero-delay timeout — because the navigation happens
after the current event returns.

### `KeyUp` does not go to the widget that got `KeyDown`

`Fl.cxx:1506` routes `FL_KEYUP` to whoever holds focus *at that moment*, and
says so in a comment. If focus moved during the `KeyDown` handling, the `KeyUp`
lands on a different widget. A `has_focus()` check in the `KeyUp` arm does not
help — by then the new owner legitimately has focus.

Any action triggered on `KeyUp` must verify it also owned the matching
`KeyDown`:

```rust
// src/ui/object_browser.rs:7732
fn consume_owned_key_up(owned_keydown: &mut Option<Key>, key: Key) -> bool {
    owned_keydown.take() == Some(key)
}
```

### Never call `take_focus()` from an `Event::Focus` handler

`Fl_Widget::take_focus()` dispatches `FL_FOCUS` **before** it sets
`Fl::focus(this)` (`Fl_Widget.cxx:150`):

```cpp
if (!handle(FL_FOCUS)) return 0;      // handler runs here
if (contains(Fl::focus())) return 1;
Fl::focus(this);                      // focus is only set now
```

So inside an `Event::Focus` arm, `has_focus()` is still `false` for the widget
being focused. A handler shaped like
`Event::Focus => if !w.has_focus() { w.take_focus() }` therefore recurses
unconditionally and overflows the stack (~2600 frames before the guard page).

There is nothing to do in an `Event::Focus` arm for a standard input:
`Fl_Input_::handle(FL_FOCUS)` already returns 1 (`Fl_Input_.cxx:1180`), and
`Fl_Input::handle(FL_PUSH)` already takes focus on click
(`Fl_Input.cxx:658`). To force focus, call `take_focus()` from *outside* the
event, and guard against re-entry (`TableBrowseFilterBar::retain_input_focus`,
`src/ui/table_browse.rs:799`).

### `deactivate()` throws focus to the group's `savedfocus_`

`Fl_Widget::deactivate()` calls `fl_throw_focus()` (`Fl_Widget.cxx:239`), which
clears `Fl::focus_` and runs `fl_fix_focus()`. That re-focuses the top window,
and `Fl_Group::handle(FL_FOCUS)` restores `savedfocus_` first
(`Fl_Group.cxx:156`) — the widget that had focus before the current one, often
in a completely different pane.

So disabling a focused input while a background operation runs silently hands
focus somewhere else, and `take_focus()` cannot take it back until the widget
is active again (`Fl_Widget::take_focus` bails on `!takesevents()`,
`Fl_Widget.cxx:151`). Record the focused widget before deactivating and restore
it on reactivation; see `TableBrowseFilterBar::set_active`
(`src/ui/table_browse.rs:1053`).

### Escape closes the window

Unhandled `FL_SHORTCUT` with `FL_Escape` invokes the window callback
(`Fl.cxx:1567`). Handle Escape explicitly wherever it should mean "dismiss the
popup" instead.

### Arrow/Tab keys navigate

`Fl_Group::handle(FL_KEYBOARD)` returns `navigation(navkey())`. An unhandled
arrow key inside a group moves focus rather than doing nothing.

### Keyboard layout

Use both `app::event_key()` and `app::event_original_key()` when matching
shortcut keys; a non-US layout reports a different logical key. This repo
funnels that through `shortcut_key_for_layout`
(`src/ui/table_browse.rs:488`).

### IME composition

While a CJK input method is composing, `app::compose_state() > 0`. Showing or
hiding a toplevel (popup) during composition steals the key window on macOS and
aborts the composition. Guard any typing-path window transition:

```rust
// src/ui/intellisense.rs:3253
if self.window.shown() && fltk::app::compose_state() > 0 { return; }
```

See `src/ui/sql_editor/hangul_repair.rs` for the recovery path.

## 6. Teardown

Closures capture `Arc<Mutex<…>>` state and keep the widget tree alive. Detach
them explicitly when the owning component closes:

```rust
// src/ui/result_table.rs:9324
self.table.handle(|_, _| false);
self.table.resize_callback(|_, _, _, _, _| {});
self.table.draw_cell(|_, _, _, _, _, _, _, _| {});
```

Same pattern in `TableBrowse::cleanup_for_close` (`src/ui/table_browse.rs:1073`)
and `ObjectBrowserWidget::drop` (`src/ui/object_browser.rs:7688`).

Two rules for detaching:

- **Only the last owner may detach.** fltk-rs widget handles are cheap clones
  sharing one C++ widget, so a dropped temporary clone would otherwise disable
  the live widget. `ObjectBrowserWidget::drop` guards with
  `Arc::strong_count(&self.poll_lifecycle) != 1`.
- **Check `was_deleted()` in deferred work.** Anything reached from
  `app::add_timeout` / `awake_callback` must verify the widget still exists
  before touching it (`src/ui/table_browse.rs:873`).

## 7. Checklist for a new handler

1. Which event classes do I need? Does FLTK already route them to me (§1)?
2. Do I need to pre-empt the native handler, or observe it? Set
   `super_handle_first` accordingly and leave a comment saying why.
3. Is a handler already installed on this widget (theme hover, tab strip)?
4. Keyboard arms: gate on `has_focus()`. `Shortcut` arms: gate on `has_focus()`.
5. Want `Drag`/`Released`? Return `true` from `Push`. Want `Move`/`Leave`?
   Return `true` from `Enter`.
6. Every unhandled path returns `false`.
7. Hit tests extracted into pure, unit-tested functions.
8. Detach in `cleanup`/`drop`, guarded by ownership; `was_deleted()` in
   deferred callbacks.
