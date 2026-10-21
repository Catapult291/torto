use super::*;
use egui::{
    text::{CCursor, CCursorRange},
    *,
};
use windows::Win32::UI::Input::Ime::{HIMC, ImmGetContext, ImmReleaseContext};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_IMMENABLED};
use windows::core::{Result, implement};
use winit::platform::windows::EventLoopBuilderExtWindows;

fn ime_context(hwnd: HWND) -> HIMC {
    let ime = unsafe { ImmGetContext(hwnd) };
    if !ime.is_invalid() {
        assert!(unsafe { ImmReleaseContext(hwnd, ime) }.as_bool());
    }
    ime
}

#[implement(ITfEditSession)]
struct ReplaceSession {
    context: ITfContext,
    expected: String,
    replacement: String,
    called: Rc<RefCell<bool>>,
}
impl ITfEditSession_Impl for ReplaceSession_Impl {
    fn DoEditSession(&self, cookie: u32) -> Result<()> {
        unsafe {
            let range = self.context.GetStart(cookie)?;
            let end = self.context.GetEnd(cookie)?;
            range.ShiftEndToRange(cookie, &end, TF_ANCHOR_END)?;
            let cursor = range.Clone()?;
            let mut text = Vec::new();
            loop {
                let mut buffer = [0u16; 511];
                let mut count = 0;
                cursor.GetText(cookie, TF_TF_MOVESTART, &mut buffer, &mut count)?;
                if count == 0 {
                    break;
                }
                text.extend_from_slice(&buffer[..count as usize]);
            }
            if String::from_utf16(&text).map_err(|_| E_FAIL)? != self.expected {
                return Err(E_FAIL.into());
            }
            let replacement = self.replacement.encode_utf16().collect::<Vec<_>>();
            range.SetText(cookie, 0, &replacement)?;
            *self.called.borrow_mut() = true;
            Ok(())
        }
    }
}

#[test]
#[allow(deprecated)]
fn tsf_real_context_reads_and_replaces_egui_text_and_isolates_old_editor_ranges() -> Result<()> {
    let mut builder = winit::event_loop::EventLoop::builder();
    builder.with_any_thread(true);
    let event_loop = builder.build().map_err(|_| E_FAIL)?;
    // Never show or focus a native window: this tests the actual TSF context on
    // its own thread without stealing keyboard focus or sending user data.
    let window = Arc::new(
        event_loop
            .create_window(winit::window::Window::default_attributes().with_visible(false))
            .map_err(|_| E_FAIL)?,
    );
    let mut native = TextInput::new(window).map_err(|_| E_FAIL)?;
    let id = Id::new("editor-a");
    let original = "Original 中文🙂\n".repeat(600);
    let replacement = "Translated 译文🙂\n".repeat(600);
    let snapshot = Snapshot {
        id,
        text: original.clone().into(),
        selection: CCursorRange::one(CCursor::new(3)),
        rect: Rect::from_min_size(Pos2::ZERO, vec2(500.0, 200.0)),
        cursor_rect: Rect::from_min_size(pos2(20.0, 10.0), vec2(1.0, 20.0)),
        multiline: true,
        char_limit: usize::MAX,
    };
    native.sync(Some(snapshot), 1.0);
    assert!(
        native.data.borrow().active,
        "TSF association must succeed even without a previous document"
    );
    assert_eq!(
        unsafe { native.context.GetStatus()? }.dwDynamicFlags,
        0,
        "TSF must observe the focused editor as writable"
    );
    let hwnd = native.data.borrow().hwnd;
    // A writable TSF store alone is not enough for hybrid IMM/TSF input methods.
    // winit starts with the window's IMM context disabled; it must be restored
    // when the native editor takes over, even though legacy IME events are filtered.
    if unsafe { GetSystemMetrics(SM_IMMENABLED) } != 0 {
        let ime = ime_context(hwnd);
        assert!(
            !ime.is_invalid(),
            "the focused native editor must enable the window IME context"
        );
    }
    let old_context = native.context.clone();
    let called = Rc::new(RefCell::new(false));
    let session: ITfEditSession = ReplaceSession {
        context: old_context.clone(),
        expected: original.clone(),
        replacement: replacement.clone(),
        called: called.clone(),
    }
    .into();
    unsafe {
        native
            .context
            .RequestEditSession(native.client, &session, TF_ES_SYNC | TF_ES_READWRITE)?
            .ok()?;
    }
    assert!(*called.borrow());
    assert_eq!(
        String::from_utf16(&native.data.borrow().text).unwrap(),
        replacement
    );

    let ctx = Context::default();
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0))),
        ..Default::default()
    };
    native.begin_frame(&ctx, &mut input);
    ctx.memory_mut(|memory| memory.request_focus(id));
    let mut actual = original.clone();
    let mut output = ctx.run_ui(input, |ui| {
        ui.add(TextEdit::multiline(&mut actual).id(id));
    });
    output.textures_delta.clear();
    assert_eq!(actual, replacement);

    let other = Snapshot {
        id: Id::new("editor-b"),
        text: "another editor".into(),
        ..native_text_input::snapshot(&ctx).unwrap()
    };
    native.sync(Some(other), 1.0);
    let session: ITfEditSession = ReplaceSession {
        context: old_context.clone(),
        expected: replacement,
        replacement: "must not replace editor B".into(),
        called: Rc::new(RefCell::new(false)),
    }
    .into();
    let stale = unsafe {
        old_context.RequestEditSession(native.client, &session, TF_ES_SYNC | TF_ES_READWRITE)
    };
    assert!(stale.is_err() || stale.is_ok_and(|result| result.is_err()));
    assert_eq!(
        String::from_utf16(&native.data.borrow().text).unwrap(),
        "another editor"
    );
    // Composition state is persistent, but metadata is event-driven. Repeating
    // a synthetic Preedit every idle frame would keep egui repainting forever.
    native.data.borrow_mut().composition = Some(1..4);
    native.data.borrow_mut().dirty = true;
    let mut first = RawInput::default();
    native.begin_frame(&ctx, &mut first);
    assert!(first.events.is_empty());
    assert!(native_text_input::blocks_submit(&ctx));
    let mut idle = RawInput::default();
    native.begin_frame(&ctx, &mut idle);
    assert!(idle.events.is_empty());
    assert!(native_text_input::blocks_submit(&ctx));
    native.data.borrow_mut().composition = None;
    let snapshot = native.data.borrow().snapshot.clone().unwrap();
    let context = native.context.clone();
    let ime = ime_context(hwnd);
    native.sync(Some(snapshot.clone()), 1.0);
    assert_eq!(
        native.context, context,
        "idle frames must keep the same TSF document"
    );
    assert_eq!(
        ime_context(hwnd),
        ime,
        "idle frames must not reset the IME context"
    );
    native.set_focused(false);
    assert!(!native.data.borrow().active);
    assert!(
        ime_context(hwnd).is_invalid(),
        "detaching must release native IME ownership"
    );
    native.sync(Some(snapshot), 1.0);
    assert!(native.data.borrow().active);
    if unsafe { GetSystemMetrics(SM_IMMENABLED) } != 0 {
        assert!(
            !ime_context(hwnd).is_invalid(),
            "refocusing must restore IME input"
        );
    }
    Ok(())
}
