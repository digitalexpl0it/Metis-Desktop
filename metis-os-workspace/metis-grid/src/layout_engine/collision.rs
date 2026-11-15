use super::LayoutItem;

/// Two items collide if their bounding boxes overlap and they are not the same item.
pub fn collides(a: &LayoutItem, b: &LayoutItem) -> bool {
    if a.id == b.id {
        return false;
    }
    if a.right() <= b.col {
        return false;
    }
    if a.col >= b.right() {
        return false;
    }
    if a.bottom() <= b.row {
        return false;
    }
    a.row < b.bottom()
}

pub fn get_first_collision<'a>(
    layout: &'a [LayoutItem],
    item: &LayoutItem,
) -> Option<&'a LayoutItem> {
    layout.iter().find(|other| collides(other, item))
}

pub fn get_all_collisions<'a>(layout: &'a [LayoutItem], item: &LayoutItem) -> Vec<&'a LayoutItem> {
    layout
        .iter()
        .filter(|other| collides(other, item))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_engine::LayoutItem;

    fn item(id: &str, col: u32, row: u32, w: u32, h: u32) -> LayoutItem {
        LayoutItem {
            id: id.into(),
            col,
            row,
            w,
            h,
            pinned: false,
            moved: false,
        }
    }

    #[test]
    fn adjacent_tiles_do_not_collide() {
        let a = item("a", 0, 0, 3, 2);
        let b = item("b", 3, 0, 3, 2);
        assert!(!collides(&a, &b));
    }

    #[test]
    fn overlapping_tiles_collide() {
        let a = item("a", 0, 0, 4, 4);
        let b = item("b", 2, 2, 4, 4);
        assert!(collides(&a, &b));
    }
}
