//! Windows TSF host for egui TextEdit. All COM access is confined to the window
//! thread; there is no polling, global keyboard hook, clipboard or worker I/O.
//! Keyboard dispatch stays with Windows/winit's IMM/TSF bridge. A second
//! ITfKeystrokeMgr message filter processes hybrid IMEs' Shift transitions twice.
#![cfg(windows)]

mod store;
#[cfg(test)]
mod tests;
use egui::native_text_input::{self, Edit, Snapshot};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use store::{Data, Store};
use windows::{
    Win32::{Foundation::*, System::Com::*, UI::TextServices::*},
    core::Interface,
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

pub struct TextInput {
    client: u32,
    failed: bool,
    data: Rc<RefCell<Data>>,
    manager: ITfThreadMgr,
    document: ITfDocumentMgr,
    context: ITfContext,
    previous: Option<ITfDocumentMgr>,
    _store: ITextStoreACP,
    _window: Arc<Window>,
    _apartment: Apartment,
}
struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

impl TextInput {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let RawWindowHandle::Win32(handle) =
            window.window_handle().map_err(|e| e.to_string())?.as_raw()
        else {
            return Err("not a Win32 window".into());
        };
        // SAFETY: all objects remain on the creating UI thread; Arc<Window>
        // retains the HWND, and Drop deactivates TSF before balancing COM.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
            let apartment = Apartment;
            let result = (|| -> windows::core::Result<Self> {
                let manager: ITfThreadMgr =
                    CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
                let client = manager.Activate()?;
                let document = manager.CreateDocumentMgr()?;
                let wake = window.clone();
                let data = Rc::new(RefCell::new(Data::new(
                    HWND(handle.hwnd.get() as *mut _),
                    Box::new(move || wake.request_redraw()),
                )));
                let store: ITextStoreACP = Store { data: data.clone() }.into();
                let mut context = None;
                let mut cookie = 0;
                if let Err(error) =
                    document.CreateContext(client, 0, &store, &mut context, &mut cookie)
                {
                    let _ = manager.Deactivate();
                    return Err(error);
                }
                let context = context.ok_or(E_FAIL)?;
                if let Err(error) = document.Push(&context) {
                    let _ = manager.Deactivate();
                    return Err(error);
                }
                Ok(Self {
                    client,
                    failed: false,
                    data,
                    manager,
                    document,
                    context,
                    previous: None,
                    _store: store,
                    _window: window.clone(),
                    _apartment: apartment,
                })
            })();
            result.map_err(|e| e.to_string())
        }
    }

    pub fn begin_frame(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        let mut data = self.data.borrow_mut();
        let owner = data
            .active
            .then(|| data.snapshot.as_ref().map(|s| s.id))
            .flatten();
        let edit = if data.dirty {
            data.dirty = false;
            data.snapshot.as_ref().map(|snapshot| Edit {
                id: snapshot.id,
                before: snapshot.text.clone(),
                text: String::from_utf16_lossy(&data.text).into(),
                selection: store::selection(&data),
                composition: data.composition.as_ref().map(|r| {
                    store::char_index(&data.text, r.start)..store::char_index(&data.text, r.end)
                }),
            })
        } else {
            None
        };
        if data.active {
            // IMM and TSF must not both deliver the same composing string.
            input
                .events
                .retain(|event| !matches!(event, egui::Event::Ime(_)));
        }
        native_text_input::begin_frame(ctx, !self.failed, owner, edit);
        native_text_input::set_composing(ctx, data.active && data.composition.is_some());
    }

    pub fn end_frame(&mut self, ctx: &egui::Context, scale: f32) -> bool {
        let snapshot = native_text_input::snapshot(ctx)
            .filter(|s| s.text.encode_utf16().count() <= store::MAX_UNITS);
        let focused = self._window.has_focus();
        self.sync(snapshot.filter(|_| focused), scale);
        self.data.borrow().active
    }

    pub fn set_focused(&mut self, focused: bool) {
        if !focused {
            self.detach();
        }
    }

    fn sync(&mut self, snapshot: Option<Snapshot>, scale: f32) {
        if self.failed {
            return;
        }
        let new_id = snapshot.as_ref().map(|s| s.id);
        let old_id = self.data.borrow().snapshot.as_ref().map(|s| s.id);
        if new_id != old_id {
            self.detach();
            if new_id.is_some() && self.replace_document().is_err() {
                self.failed = true;
                return;
            }
            self.data.borrow_mut().composition = None;
        }
        let (sink, text_changed, selection_changed, layout_changed, change) = {
            let mut data = self.data.borrow_mut();
            let old_len = data.text.len() as i32;
            let text = snapshot
                .as_ref()
                .map(|s| s.text.encode_utf16().collect::<Vec<_>>())
                .unwrap_or_default();
            let (start, end, active_end) = snapshot
                .as_ref()
                .map(|s| {
                    let a = store::utf16_index(&s.text, s.selection.primary.index.into()) as i32;
                    let b = store::utf16_index(&s.text, s.selection.secondary.index.into()) as i32;
                    (
                        a.min(b),
                        a.max(b),
                        if a < b { TS_AE_START } else { TS_AE_END },
                    )
                })
                .unwrap_or((0, 0, TS_AE_END));
            let text_changed = data.text != text;
            let selection_changed =
                (data.start, data.end, data.active_end) != (start, end, active_end);
            let layout_changed = data.snapshot.as_ref().map(|s| (s.rect, s.cursor_rect))
                != snapshot.as_ref().map(|s| (s.rect, s.cursor_rect))
                || data.scale != scale;
            data.text = text;
            data.start = start;
            data.end = end;
            data.active_end = active_end;
            data.scale = scale;
            data.snapshot = snapshot;
            (
                data.sink.clone(),
                text_changed,
                selection_changed,
                layout_changed,
                TS_TEXTCHANGE {
                    acpStart: 0,
                    acpOldEnd: old_len,
                    acpNewEnd: data.text.len() as i32,
                },
            )
        };
        // SAFETY: no borrow survives COM calls. Notifications describe application
        // changes only: TSF-originated writes already match and are not echoed.
        unsafe {
            if new_id.is_some() && !self.data.borrow().active {
                let hwnd = self.data.borrow().hwnd;
                // winit creates windows with IME disabled. Restoring the native
                // context is also required by hybrid IMM/TSF input methods for
                // their candidate UI and Shift mode switching. Do this before
                // associating our document: enabling IMM can change TSF focus.
                // Never re-associate IMM on idle frames or composition updates.
                self._window.set_ime_allowed(true);
                // SetFocus can synchronously activate the TIP, which must see a
                // writable store during its OnSetFocus callback, not one frame later.
                self.data.borrow_mut().active = true;
                if let Ok(previous) = associate_focus(&self.manager, hwnd, Some(&self.document)) {
                    self.previous = previous;
                    if self.manager.SetFocus(&self.document).is_err() {
                        self.detach();
                        self.failed = true;
                    }
                } else {
                    self.data.borrow_mut().active = false;
                    self._window.set_ime_allowed(false);
                    self.failed = true;
                }
            }
            if let Some(sink) = sink {
                let mask = self.data.borrow().mask;
                if text_changed && mask & TS_AS_TEXT_CHANGE != 0 {
                    let _ = sink.OnTextChange(TEXT_STORE_TEXT_CHANGE_FLAGS(0), &change);
                }
                if selection_changed && mask & TS_AS_SEL_CHANGE != 0 {
                    let _ = sink.OnSelectionChange();
                }
                if layout_changed && mask & TS_AS_LAYOUT_CHANGE != 0 {
                    let _ = sink.OnLayoutChange(TS_LC_CHANGE, 0);
                }
            }
        }
    }

    // Each editor gets a distinct COM document. An external service retaining a
    // range from a previous editor sees that old, read-only store, never the new
    // editor's text. Swapping just a widget id in one text store is insufficient.
    fn replace_document(&mut self) -> windows::core::Result<()> {
        unsafe {
            let document = self.manager.CreateDocumentMgr()?;
            let wake = self._window.clone();
            let hwnd = self.data.borrow().hwnd;
            let data = Rc::new(RefCell::new(Data::new(
                hwnd,
                Box::new(move || wake.request_redraw()),
            )));
            let store: ITextStoreACP = Store { data: data.clone() }.into();
            let mut context = None;
            let mut cookie = 0;
            document.CreateContext(self.client, 0, &store, &mut context, &mut cookie)?;
            let context = context.ok_or(E_FAIL)?;
            document.Push(&context)?;
            let _ = self.document.Pop(TF_POPF_ALL);
            self.document = document;
            self.context = context;
            self._store = store;
            self.data = data;
            Ok(())
        }
    }

    fn detach(&mut self) {
        let active = self.data.borrow().active;
        if !active {
            return;
        }
        // SAFETY: cancel the old composing session before changing document owner.
        unsafe {
            if let Ok(compositions) = self.context.cast::<ITfContextOwnerCompositionServices>() {
                let _ = compositions.TerminateComposition(None);
            }
            self.data.borrow_mut().active = false;
            let hwnd = self.data.borrow().hwnd;
            let _ = associate_focus(&self.manager, hwnd, self.previous.as_ref());
            let _ = self.manager.SetFocus(self.previous.as_ref());
        }
        self._window.set_ime_allowed(false);
    }
}
// AssociateFocus may successfully return a null previous document. The generated
// non-optional return wrapper treats that case as E_POINTER; preserve S_OK here.
unsafe fn associate_focus(
    manager: &ITfThreadMgr,
    hwnd: HWND,
    document: Option<&ITfDocumentMgr>,
) -> windows::core::Result<Option<ITfDocumentMgr>> {
    let mut previous = std::ptr::null_mut();
    unsafe {
        (Interface::vtable(manager).AssociateFocus)(
            Interface::as_raw(manager),
            hwnd,
            document.map_or(std::ptr::null_mut(), Interface::as_raw),
            &mut previous,
        )
        .ok()?;
        Ok((!previous.is_null()).then(|| ITfDocumentMgr::from_raw(previous)))
    }
}
impl Drop for TextInput {
    fn drop(&mut self) {
        self.detach();
        // SAFETY: balancing references/activation on the same STA as new().
        unsafe {
            let _ = self.document.Pop(TF_POPF_ALL);
            let _ = self.manager.Deactivate();
        }
    }
}
