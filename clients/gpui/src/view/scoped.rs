//! What a frame records as it draws (control bounds, drawn texts, card slots), kept per view that
//! drew it. A pane body that GPUI replays from its cache runs none of its draw closures, so its
//! entries from the frame that drew it stay until that body draws again.
use std::cell::{Ref, RefCell};
use std::rc::Rc;

/// The scope of the window's own frame, outside every pane body.
pub const SHELL: &str = "";

pub struct Scoped<T> {
    current: RefCell<Rc<str>>,
    inner: RefCell<Inner<T>>,
}

struct Inner<T> {
    items: Vec<T>,
    scopes: Vec<Rc<str>>,
}

impl<T> Default for Scoped<T> {
    fn default() -> Self {
        Self { current: RefCell::new(Rc::from(SHELL)), inner: RefCell::new(Inner { items: vec![], scopes: vec![] }) }
    }
}

impl<T> Scoped<T> {
    /// The scope a draw closure made now records into.
    pub fn scope(&self) -> Rc<str> {
        self.current.borrow().clone()
    }
    /// Draw into `scope` until the next call; a body sets its pane, then the shell again.
    pub fn enter(&self, scope: &str) {
        *self.current.borrow_mut() = Rc::from(scope);
    }
    pub fn push(&self, scope: &Rc<str>, item: T) {
        let mut inner = self.inner.borrow_mut();
        inner.items.push(item);
        inner.scopes.push(scope.clone());
    }
    /// Forget what `scope` drew, before it draws again.
    pub fn clear(&self, scope: &str) {
        self.retain_scopes(|s| s != scope);
    }
    /// Keep only the scopes `keep` accepts: a pane gone from the window takes its entries with it.
    pub fn retain_scopes(&self, keep: impl Fn(&str) -> bool) {
        let mut inner = self.inner.borrow_mut();
        let Inner { items, scopes } = &mut *inner;
        let mut i = 0;
        items.retain(|_| {
            let k = keep(&scopes[i]);
            i += 1;
            k
        });
        scopes.retain(|s| keep(s));
    }
    /// Every entry, of every scope.
    pub fn borrow(&self) -> Ref<'_, Vec<T>> {
        Ref::map(self.inner.borrow(), |i| &i.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_clears_alone_and_a_gone_pane_goes() {
        let s: Scoped<u32> = Scoped::default();
        let shell = s.scope();
        s.push(&shell, 1);
        s.enter("p1");
        let p1 = s.scope();
        s.push(&p1, 2);
        s.enter("p2");
        s.push(&s.scope(), 3);
        s.clear("p1");
        assert_eq!(*s.borrow(), vec![1, 3]);
        s.retain_scopes(|scope| scope == SHELL);
        assert_eq!(*s.borrow(), vec![1]);
    }
}
