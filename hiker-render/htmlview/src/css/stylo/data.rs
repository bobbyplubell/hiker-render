//! Per-element Stylo cascade storage.
//!
//! Copied near-verbatim from the proven `stylo-spike/src/stylo_data.rs`, which
//! is itself adapted from blitz `packages/blitz-dom/src/node/stylo_data.rs`.
//! Interior-mutable wrapper around `Option<ElementDataWrapper>` where Stylo
//! stores the cascade result (the primary `Arc<ComputedValues>`) for an element.

use std::cell::UnsafeCell;
use std::fmt;
use std::ops::{Deref, DerefMut};

use style::data::{ElementDataMut, ElementDataRef, ElementDataWrapper};
use style::servo_arc::Arc;

/// We don't do incremental relayout, so every restyle is "all damage".
/// blitz defines this in its layout::damage module; we inline the bit pattern.
use style::selector_parser::RestyleDamage;
const ALL_DAMAGE: RestyleDamage = RestyleDamage::from_bits_retain(0b_0000_0000_0111_1111);

pub struct StyloData {
    inner: UnsafeCell<Option<ElementDataWrapper>>,
}

impl Default for StyloData {
    fn default() -> Self {
        Self {
            inner: UnsafeCell::new(None),
        }
    }
}

impl fmt::Debug for StyloData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StyloData").finish_non_exhaustive()
    }
}

impl Deref for StyloData {
    type Target = Option<ElementDataWrapper>;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.inner.get() }
    }
}

impl DerefMut for StyloData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.get_mut()
    }
}

impl StyloData {
    pub fn has_data(&self) -> bool {
        unsafe { &*self.inner.get() }.is_some()
    }

    pub fn get(&self) -> Option<ElementDataRef<'_>> {
        self.as_ref().map(|w| w.borrow())
    }

    pub fn primary_styles(&self) -> Option<StyleDataRef<'_>> {
        let stylo_element_data = self.get();
        if stylo_element_data
            .as_ref()
            .and_then(|d| d.styles.get_primary())
            .is_some()
        {
            Some(StyleDataRef(self.get().unwrap()))
        } else {
            None
        }
    }

    pub unsafe fn unsafe_stylo_only_mut(&self) -> Option<ElementDataMut<'_>> {
        let opt = unsafe { &mut *self.inner.get() };
        opt.as_mut().map(|w| w.borrow_mut())
    }

    /// SAFETY: no outstanding borrows to this container when called.
    pub unsafe fn ensure_init(&self) -> ElementDataMut<'_> {
        if !self.has_data() {
            unsafe { *self.inner.get() = Some(ElementDataWrapper::default()) };
            let mut data_mut = unsafe { self.unsafe_stylo_only_mut() }.unwrap();
            data_mut.damage = ALL_DAMAGE;
            data_mut
        } else {
            unsafe { self.unsafe_stylo_only_mut() }.unwrap()
        }
    }

    /// SAFETY: no outstanding borrows to this container when called.
    pub unsafe fn clear(&self) {
        unsafe { *self.inner.get() = None };
    }
}

pub struct StyleDataRef<'a>(ElementDataRef<'a>);

impl Deref for StyleDataRef<'_> {
    type Target = Arc<style::properties::ComputedValues>;

    fn deref(&self) -> &Self::Target {
        self.0.styles.get_primary().unwrap()
    }
}
