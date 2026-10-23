pub struct Ledger<T> {
    next: u32,
    items: Vec<(u32, T)>,
}

impl<T> Default for Ledger<T> {
    fn default() -> Self {
        Self {
            next: 1,
            items: Vec::new(),
        }
    }
}

impl<T> Ledger<T> {
    pub fn add(&mut self, item: T) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.items.push((id, item));
        id
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.items
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, v)| v)
    }

    pub fn take(&mut self, id: u32) -> Option<T> {
        let at = self.items.iter().position(|(i, _)| *i == id)?;
        Some(self.items.remove(at).1)
    }

    pub fn take_last(&mut self) -> Option<(u32, T)> {
        self.items.pop()
    }

    pub fn drain(&mut self) -> Vec<(u32, T)> {
        std::mem::take(&mut self.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_grow_and_never_repeat() {
        let mut l = Ledger::default();
        let a = l.add("a");
        let b = l.add("b");
        assert!(b > a);
        assert_eq!(l.take(a), Some("a"));
        let c = l.add("c");
        assert!(c > b);
    }

    #[test]
    fn take_removes_only_that_entry() {
        let mut l = Ledger::default();
        let a = l.add("a");
        let b = l.add("b");
        assert_eq!(l.take(a), Some("a"));
        assert_eq!(l.take(a), None);
        assert_eq!(l.get_mut(a), None);
        assert_eq!(l.get_mut(b), Some(&mut "b"));
        assert_eq!(l.take(b), Some("b"));
        assert_eq!(l.take_last(), None);
    }

    #[test]
    fn undo_picks_the_newest_first() {
        let mut l = Ledger::default();
        let a = l.add("a");
        let b = l.add("b");
        assert_eq!(l.take_last(), Some((b, "b")));
        assert_eq!(l.take_last(), Some((a, "a")));
        assert_eq!(l.take_last(), None);
    }

    #[test]
    fn drain_keeps_order_and_empties() {
        let mut l = Ledger::default();
        l.add(1);
        l.add(2);
        l.add(3);
        let out: Vec<i32> = l.drain().into_iter().map(|(_, v)| v).collect();
        assert_eq!(out, vec![1, 2, 3]);
        assert_eq!(l.drain().len(), 0);
    }
}
