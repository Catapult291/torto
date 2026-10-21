use egui::{
    native_text_input::Snapshot,
    text::{CCursor, CCursorRange},
};
use std::{cell::RefCell, ops::Range, rc::Rc};
use windows::Win32::System::Ole::{
    CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::ClientToScreen,
        System::Com::{FORMATETC, IDataObject},
        UI::TextServices::*,
    },
    core::*,
};

pub(crate) const MAX_UNITS: usize = 1_048_576;
pub(crate) struct Data {
    pub hwnd: HWND,
    pub snapshot: Option<Snapshot>,
    pub active: bool,
    pub text: Vec<u16>,
    pub start: i32,
    pub end: i32,
    pub active_end: TsActiveSelEnd,
    pub composition: Option<Range<usize>>,
    pub dirty: bool,
    pub sink: Option<ITextStoreACPSink>,
    pub mask: u32,
    lock_flags: u32,
    deferred: u32,
    pub scale: f32,
    wake: Box<dyn Fn()>,
}
impl Data {
    pub fn new(hwnd: HWND, wake: Box<dyn Fn()>) -> Self {
        Self {
            hwnd,
            snapshot: None,
            active: false,
            text: Vec::new(),
            start: 0,
            end: 0,
            active_end: TS_AE_END,
            composition: None,
            dirty: false,
            sink: None,
            mask: 0,
            lock_flags: 0,
            deferred: 0,
            scale: 1.0,
            wake,
        }
    }
    fn readable(&self) -> Result<()> {
        if self.lock_flags & TS_LF_READ.0 == 0 {
            Err(TS_E_NOLOCK.into())
        } else {
            Ok(())
        }
    }
    fn writable(&self) -> Result<()> {
        self.readable()?;
        if self.lock_flags & TS_LF_READWRITE.0 != TS_LF_READWRITE.0 {
            return Err(TS_E_NOLOCK.into());
        }
        if !self.active || self.snapshot.is_none() {
            return Err(TS_E_READONLY.into());
        }
        Ok(())
    }
    fn range(&self, start: i32, end: i32) -> Result<Range<usize>> {
        let end = if end == -1 {
            self.text.len() as i32
        } else {
            end
        };
        if start < 0 || start > end || end as usize > self.text.len() {
            return Err(TS_E_INVALIDPOS.into());
        }
        let range = start as usize..end as usize;
        Ok(range)
    }
    fn edit_range(&self, start: i32, end: i32) -> Result<Range<usize>> {
        let range = self.range(start, end)?;
        if !boundary(&self.text, range.start) || !boundary(&self.text, range.end) {
            return Err(TS_E_INVALIDPOS.into());
        }
        Ok(range)
    }
}
fn boundary(text: &[u16], offset: usize) -> bool {
    offset <= text.len()
        && (offset == 0
            || offset == text.len()
            || !(0xD800..=0xDBFF).contains(&text[offset - 1])
            || !(0xDC00..=0xDFFF).contains(&text[offset]))
}
pub(crate) fn utf16_index(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}
pub(crate) fn char_index(text: &[u16], units: usize) -> usize {
    char::decode_utf16(text.iter().copied().take(units)).count()
}
pub(crate) fn selection(data: &Data) -> CCursorRange {
    let start = CCursor::new(char_index(&data.text, data.start as usize));
    let end = CCursor::new(char_index(&data.text, data.end as usize));
    if data.active_end == TS_AE_START {
        CCursorRange {
            primary: start,
            secondary: end,
            h_pos: None,
        }
    } else {
        CCursorRange {
            primary: end,
            secondary: start,
            h_pos: None,
        }
    }
}

#[implement(ITextStoreACP, ITfContextOwnerCompositionSink)]
pub(crate) struct Store {
    pub data: Rc<RefCell<Data>>,
}

#[cfg(test)]
mod tests;

// All buffer access below follows the TSF document-lock protocol. COM callbacks
// are made only after dropping RefCell borrows, including nested lock upgrades.
impl ITextStoreACP_Impl for Store_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Ref<IUnknown>, mask: u32) -> Result<()> {
        if riid.is_null() {
            return Err(E_POINTER.into());
        }
        if unsafe { *riid } != ITextStoreACPSink::IID {
            return Err(CONNECT_E_CANNOTCONNECT.into());
        }
        let sink: ITextStoreACPSink = punk.ok()?.cast()?;
        let mut data = self.data.borrow_mut();
        if data.sink.as_ref().is_some_and(|old| old != &sink) {
            return Err(CONNECT_E_ADVISELIMIT.into());
        }
        data.sink = Some(sink);
        data.mask = mask;
        Ok(())
    }
    fn UnadviseSink(&self, punk: Ref<IUnknown>) -> Result<()> {
        let sink: ITextStoreACPSink = punk.ok()?.cast()?;
        let mut data = self.data.borrow_mut();
        if data.sink.as_ref() != Some(&sink) {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        data.sink = None;
        data.mask = 0;
        Ok(())
    }
    fn RequestLock(&self, flags: u32) -> Result<HRESULT> {
        let sink = {
            let mut data = self.data.borrow_mut();
            if flags & TS_LF_READ.0 == 0 {
                return Err(E_INVALIDARG.into());
            }
            if data.lock_flags != 0 {
                if flags & TS_LF_SYNC != 0 {
                    return Ok(TS_E_SYNCHRONOUS);
                }
                data.deferred |= flags & TS_LF_READWRITE.0;
                return Ok(TS_S_ASYNC);
            }
            data.lock_flags = flags;
            match data.sink.clone() {
                Some(s) => s,
                None => {
                    data.lock_flags = 0;
                    return Err(E_UNEXPECTED.into());
                }
            }
        };
        let result = unsafe { sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags)) };
        let deferred = {
            let mut d = self.data.borrow_mut();
            d.lock_flags = 0;
            std::mem::take(&mut d.deferred)
        };
        if deferred != 0 {
            self.data.borrow_mut().lock_flags = deferred;
            let _ = unsafe { sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(deferred)) };
            let mut d = self.data.borrow_mut();
            d.lock_flags = 0;
            d.deferred = 0;
        }
        let data = self.data.borrow();
        if data.dirty {
            (data.wake)();
        }
        Ok(result.err().map_or(HRESULT(0), |e| e.code()))
    }
    fn GetStatus(&self) -> Result<TS_STATUS> {
        let data = self.data.borrow();
        Ok(TS_STATUS {
            dwDynamicFlags: if data.active && data.snapshot.is_some() {
                0
            } else {
                TS_SD_READONLY
            },
            dwStaticFlags: TS_SS_NOHIDDENTEXT,
        })
    }
    fn QueryInsert(
        &self,
        start: i32,
        end: i32,
        count: u32,
        result_start: *mut i32,
        result_end: *mut i32,
    ) -> Result<()> {
        if result_start.is_null() || result_end.is_null() {
            return Err(E_POINTER.into());
        }
        let data = self.data.borrow();
        let range = data.edit_range(start, end)?;
        if data.text.len() - range.len() + count as usize > MAX_UNITS {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *result_start = start;
            *result_end = end;
        }
        Ok(())
    }
    fn GetSelection(
        &self,
        index: u32,
        count: u32,
        selection: *mut TS_SELECTION_ACP,
        fetched: *mut u32,
    ) -> Result<()> {
        if fetched.is_null() || count > 0 && selection.is_null() {
            return Err(E_POINTER.into());
        }
        let data = self.data.borrow();
        data.readable()?;
        unsafe {
            *fetched = 0;
        }
        if index != 0 && index != TF_DEFAULT_SELECTION {
            return Err(TS_E_NOSELECTION.into());
        }
        if count > 0 {
            unsafe {
                *selection = TS_SELECTION_ACP {
                    acpStart: data.start,
                    acpEnd: data.end,
                    style: TS_SELECTIONSTYLE {
                        ase: data.active_end,
                        fInterimChar: false.into(),
                    },
                };
                *fetched = 1;
            }
        }
        Ok(())
    }
    fn SetSelection(&self, count: u32, selection: *const TS_SELECTION_ACP) -> Result<()> {
        if selection.is_null() {
            return Err(E_POINTER.into());
        }
        if count != 1 {
            return Err(E_INVALIDARG.into());
        }
        let value = unsafe { *selection };
        let mut data = self.data.borrow_mut();
        data.writable()?;
        data.edit_range(value.acpStart, value.acpEnd)?;
        if !matches!(value.style.ase, TS_AE_START | TS_AE_END | TS_AE_NONE) {
            return Err(E_INVALIDARG.into());
        }
        data.start = value.acpStart;
        data.end = value.acpEnd;
        data.active_end = value.style.ase;
        data.dirty = true;
        Ok(())
    }
    fn GetText(
        &self,
        start: i32,
        end: i32,
        plain: PWSTR,
        capacity: u32,
        written: *mut u32,
        runs: *mut TS_RUNINFO,
        run_capacity: u32,
        run_count: *mut u32,
        next: *mut i32,
    ) -> Result<()> {
        if written.is_null()
            || run_count.is_null()
            || next.is_null()
            || capacity > 0 && plain.is_null()
            || run_capacity > 0 && runs.is_null()
        {
            return Err(E_POINTER.into());
        }
        let data = self.data.borrow();
        data.readable()?;
        let range = data.range(start, end)?;
        // ACP counts UTF-16 code units. Small caller buffers may split pairs;
        // callers concatenate chunks before decoding, so every read must progress.
        let length = if capacity == 0 && run_capacity > 0 {
            range.len()
        } else {
            range.len().min(capacity as usize)
        };
        unsafe {
            *written = 0;
            *run_count = 0;
            *next = start + length as i32;
            if capacity > 0 && length > 0 {
                std::ptr::copy_nonoverlapping(data.text[range.clone()].as_ptr(), plain.0, length);
                *written = length as u32;
            }
            if run_capacity > 0 && length > 0 {
                *runs = TS_RUNINFO {
                    uCount: length as u32,
                    r#type: TS_RT_PLAIN,
                };
                *run_count = 1;
            }
        }
        Ok(())
    }
    fn SetText(
        &self,
        _: u32,
        start: i32,
        end: i32,
        text: &PCWSTR,
        length: u32,
    ) -> Result<TS_TEXTCHANGE> {
        if length as usize > MAX_UNITS || length > 0 && text.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let insert = if length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(text.0, length as usize) }
        };
        let value = String::from_utf16(insert).map_err(|_| E_INVALIDARG)?;
        let mut data = self.data.borrow_mut();
        data.writable()?;
        let range = data.edit_range(start, end)?;
        let snapshot = data.snapshot.as_ref().ok_or(TS_E_READONLY)?;
        if !snapshot.multiline && value.contains(['\r', '\n']) {
            return Err(E_INVALIDARG.into());
        }
        if data.text.len() - range.len() + insert.len() > MAX_UNITS {
            return Err(E_INVALIDARG.into());
        }
        let chars = char_index(&data.text, data.text.len())
            - char_index(&data.text[range.clone()], range.len())
            + value.chars().count();
        if chars > snapshot.char_limit {
            return Err(E_INVALIDARG.into());
        }
        data.text.splice(range, insert.iter().copied());
        data.start = start + length as i32;
        data.end = data.start;
        data.active_end = TS_AE_END;
        data.dirty = true;
        Ok(TS_TEXTCHANGE {
            acpStart: start,
            acpOldEnd: end,
            acpNewEnd: data.end,
        })
    }
    fn GetFormattedText(&self, _: i32, _: i32) -> Result<IDataObject> {
        Err(E_NOTIMPL.into())
    }
    fn GetEmbedded(&self, _: i32, _: *const GUID, _: *const GUID) -> Result<IUnknown> {
        Err(TS_E_NOOBJECT.into())
    }
    fn QueryInsertEmbedded(&self, _: *const GUID, _: *const FORMATETC) -> Result<BOOL> {
        Ok(false.into())
    }
    fn InsertEmbedded(&self, _: u32, _: i32, _: i32, _: Ref<IDataObject>) -> Result<TS_TEXTCHANGE> {
        Err(E_NOTIMPL.into())
    }
    fn InsertTextAtSelection(
        &self,
        flags: u32,
        text: &PCWSTR,
        length: u32,
        start: *mut i32,
        end: *mut i32,
        change: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        if flags & !(TS_IAS_NOQUERY | TS_IAS_QUERYONLY) != 0
            || flags == (TS_IAS_NOQUERY | TS_IAS_QUERYONLY)
        {
            return Err(E_INVALIDARG.into());
        }
        let (a, b) = {
            let data = self.data.borrow();
            data.readable()?;
            (data.start, data.end)
        };
        if flags & TS_IAS_NOQUERY == 0 {
            if start.is_null() || end.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe {
                *start = a;
                *end = a.checked_add(length as i32).ok_or(E_INVALIDARG)?;
            }
        }
        if flags & TS_IAS_QUERYONLY != 0 {
            return Ok(());
        }
        if change.is_null() {
            return Err(E_POINTER.into());
        }
        let result = self.SetText(0, a, b, text, length)?;
        unsafe {
            *change = result;
        }
        Ok(())
    }
    fn InsertEmbeddedAtSelection(
        &self,
        _: u32,
        _: Ref<IDataObject>,
        _: *mut i32,
        _: *mut i32,
        _: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn RequestSupportedAttrs(&self, _: u32, _: u32, _: *const GUID) -> Result<()> {
        Ok(())
    }
    fn RequestAttrsAtPosition(&self, _: i32, _: u32, _: *const GUID, _: u32) -> Result<()> {
        Ok(())
    }
    fn RequestAttrsTransitioningAtPosition(
        &self,
        _: i32,
        _: u32,
        _: *const GUID,
        _: u32,
    ) -> Result<()> {
        Ok(())
    }
    fn FindNextAttrTransition(
        &self,
        start: i32,
        halt: i32,
        _: u32,
        _: *const GUID,
        _: u32,
        next: *mut i32,
        found: *mut BOOL,
        offset: *mut i32,
    ) -> Result<()> {
        if next.is_null() || found.is_null() || offset.is_null() {
            return Err(E_POINTER.into());
        }
        self.data.borrow().range(start, halt)?;
        unsafe {
            *next = halt;
            *found = false.into();
            *offset = 0;
        }
        Ok(())
    }
    fn RetrieveRequestedAttrs(&self, _: u32, _: *mut TS_ATTRVAL, fetched: *mut u32) -> Result<()> {
        if fetched.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *fetched = 0;
        }
        Ok(())
    }
    fn GetEndACP(&self) -> Result<i32> {
        let data = self.data.borrow();
        data.readable()?;
        Ok(data.text.len() as i32)
    }
    fn GetActiveView(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetACPFromPoint(&self, _: u32, _: *const POINT, _: u32) -> Result<i32> {
        Err(E_NOTIMPL.into())
    }
    fn GetTextExt(
        &self,
        view: u32,
        start: i32,
        end: i32,
        rect: *mut RECT,
        clipped: *mut BOOL,
    ) -> Result<()> {
        if rect.is_null() || clipped.is_null() {
            return Err(E_POINTER.into());
        }
        if view != 0 {
            return Err(E_INVALIDARG.into());
        }
        let data = self.data.borrow();
        data.readable()?;
        let (bounds, is_clipped) = text_bounds(&data, start, end)?;
        unsafe {
            *rect = if bounds == egui::Rect::ZERO {
                RECT::default()
            } else {
                screen_rect(&data, bounds)?
            };
            *clipped = is_clipped.into();
        }
        Ok(())
    }
    fn GetScreenExt(&self, view: u32) -> Result<RECT> {
        if view != 0 {
            return Err(E_INVALIDARG.into());
        }
        let data = self.data.borrow();
        let snapshot = data.snapshot.as_ref().ok_or(TS_E_NOLAYOUT)?;
        screen_rect(&data, snapshot.rect)
    }
    fn GetWnd(&self, view: u32) -> Result<HWND> {
        if view != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.data.borrow().hwnd)
    }
}
fn text_bounds(data: &Data, start: i32, end: i32) -> Result<(egui::Rect, bool)> {
    let range = data.range(start, end)?;
    let snapshot = data.snapshot.as_ref().ok_or(TS_E_NOLAYOUT)?;
    let at_selection = start == data.start && end == data.end;
    let in_composition = data.composition.as_ref().is_some_and(|composition| {
        range.start >= composition.start && range.end <= composition.end
    });
    let bounds = if at_selection || in_composition {
        snapshot.cursor_rect
    } else {
        snapshot.rect
    };
    // pfClipped describes text visibility, not whether this is the selection
    // or whether the caret's paint box extends past a padded editor's edge.
    // The bridge publishes visible editors and a caret anchor, not per-glyph
    // bounds. Clamp that anchor for placement without marking visible input as
    // clipped (which hides native candidates). A fully offscreen anchor is hidden.
    let visible = bounds.intersect(snapshot.rect);
    if visible.min.x > visible.max.x || visible.min.y > visible.max.y {
        Ok((egui::Rect::ZERO, true))
    } else {
        Ok((visible, false))
    }
}
fn screen_rect(data: &Data, rect: egui::Rect) -> Result<RECT> {
    let mut origin = POINT::default();
    // SAFETY: HWND is retained by the owning TextInput until after COM teardown.
    unsafe {
        ClientToScreen(data.hwnd, &mut origin).ok()?;
    }
    Ok(RECT {
        left: origin.x + (rect.left() * data.scale).round() as i32,
        top: origin.y + (rect.top() * data.scale).round() as i32,
        right: origin.x + (rect.right() * data.scale).ceil() as i32,
        bottom: origin.y + (rect.bottom() * data.scale).ceil() as i32,
    })
}
impl ITfContextOwnerCompositionSink_Impl for Store_Impl {
    fn OnStartComposition(&self, composition: Ref<ITfCompositionView>) -> Result<BOOL> {
        let range = composition_range(unsafe { composition.ok()?.GetRange()? })?;
        let mut data = self.data.borrow_mut();
        if !data.active || data.composition.is_some() {
            return Ok(false.into());
        }
        data.composition = Some(range);
        data.dirty = true;
        (data.wake)();
        Ok(true.into())
    }
    fn OnUpdateComposition(
        &self,
        composition: Ref<ITfCompositionView>,
        range: Ref<ITfRange>,
    ) -> Result<()> {
        let range = if let Some(range) = range.as_ref() {
            composition_range(range.clone())?
        } else {
            composition_range(unsafe { composition.ok()?.GetRange()? })?
        };
        let mut data = self.data.borrow_mut();
        data.composition = Some(range);
        data.dirty = true;
        (data.wake)();
        Ok(())
    }
    fn OnEndComposition(&self, _: Ref<ITfCompositionView>) -> Result<()> {
        let mut data = self.data.borrow_mut();
        data.composition = None;
        data.dirty = true;
        (data.wake)();
        Ok(())
    }
}
fn composition_range(range: ITfRange) -> Result<Range<usize>> {
    let range: ITfRangeACP = range.cast()?;
    let mut start = 0;
    let mut length = 0;
    unsafe {
        range.GetExtent(&mut start, &mut length)?;
    }
    if start < 0 || length < 0 {
        return Err(E_INVALIDARG.into());
    }
    Ok(start as usize..start as usize + length as usize)
}
