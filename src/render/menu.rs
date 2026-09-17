use super::*;

pub struct Menu {
    canvas: Canvas,
    text: Text,
    scale: usize,
    rows: Vec<MenuRow>,
    hover: Option<usize>,
    drawn: bool,
}

struct MenuRow {
    label: String,
    id: Option<usize>,
    checked: bool,
    grayed: bool,
    indented: bool,
    separator: bool,
    top: f32,
    bottom: f32,
}

impl Menu {
    pub fn new(scale: usize, items: &[MenuItem]) -> Menu {
        let mut text = Text::load(text_px(scale));
        let mut rows = Vec::new();
        add_rows(items, false, &mut rows);
        let (lh, _) = text.metrics();
        let (pad, indent) = (3.0 * scale as f32, 3.0 * scale as f32);
        let row_h = (lh * 1.45).round();
        let mark = text.advance("◆ ");
        let widest = rows
            .iter()
            .map(|r| text.advance(&r.label) + if r.indented { indent } else { 0.0 })
            .fold(0.0, f32::max);
        let width = (2.0 * pad + mark + widest + 2.0 * pad).round() as i32;
        let mut top = pad;
        for row in &mut rows {
            let tall = if row.separator { row_h * 0.6 } else { row_h };
            (row.top, row.bottom) = (top, top + tall);
            top += tall;
        }
        let height = (top + pad).round() as i32;
        Menu {
            canvas: Canvas::new(width as usize, height as usize),
            text,
            scale,
            rows,
            hover: None,
            drawn: false,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.canvas.width as i32, self.canvas.height as i32)
    }

    pub fn row_at(&self, (x, y): (i32, i32)) -> Option<usize> {
        if !(0..self.canvas.width as i32).contains(&x) || !(0..self.canvas.height as i32).contains(&y) {
            return None;
        }
        let y = y as f32;
        self.rows.iter().position(|r| r.id.is_some() && !r.grayed && !r.separator && y >= r.top && y < r.bottom)
    }

    pub fn id(&self, row: usize) -> Option<usize> {
        self.rows.get(row).and_then(|r| r.id)
    }

    pub fn set_hover(&mut self, hover: Option<usize>) {
        if hover != self.hover {
            self.hover = hover;
            self.drawn = false;
        }
    }

    pub fn frame(&mut self) -> Option<&Canvas> {
        if self.drawn {
            return None;
        }
        self.drawn = true;
        let s = self.scale as i32;
        let edge = (s / 2).max(1);
        let (pad, indent) = (3.0 * self.scale as f32, 3.0 * self.scale as f32);
        let (lh, ascent) = self.text.metrics();
        let mark = self.text.advance("◆ ");
        let (bw, bh) = (self.canvas.width as i32, self.canvas.height as i32);
        self.canvas.clear();
        self.canvas.chrome(0, 0, bw, bh, edge, 240);
        for (i, row) in self.rows.iter().enumerate() {
            if row.separator {
                let y = ((row.top + row.bottom) / 2.0).round() as i32;
                self.canvas.rect(pad as i32, y, bw - 2 * pad as i32, edge, NEON_CYAN, 90);
                continue;
            }
            let lit = self.hover == Some(i);
            if lit {
                self.canvas.rect(edge, row.top.round() as i32, bw - 2 * edge, (row.bottom - row.top).round() as i32, NEON_CYAN, 46);
            }
            let x = pad + mark + if row.indented { indent } else { 0.0 };
            let baseline = row.top + (row.bottom - row.top - lh) / 2.0 + ascent;
            let colour = match (row.id.is_some(), row.grayed, lit) {
                (_, true, _) => DIM,
                (false, _, _) => DIM,
                (_, _, true) => NEON_CYAN,
                _ => TEXT,
            };
            if row.checked {
                self.text.draw(&mut self.canvas, "◆", pad, baseline, NEON_CYAN, 255);
            }
            let label = row.label.clone();
            self.text.draw(&mut self.canvas, &label, x, baseline, colour, 255);
        }
        Some(&self.canvas)
    }
}

fn add_rows(items: &[MenuItem], indented: bool, rows: &mut Vec<MenuRow>) {

    let row = |label: String, id: Option<usize>, checked: bool, grayed: bool, separator: bool| MenuRow {
        label,
        id,
        checked,
        grayed,
        indented,
        separator,
        top: 0.0,
        bottom: 0.0,
    };
    for item in items {
        match item {
            MenuItem::Item { id, label, checked, grayed } => rows.push(row(label.clone(), Some(*id), *checked, *grayed, false)),
            MenuItem::Separator => rows.push(row(String::new(), None, false, false, true)),
            MenuItem::Submenu { label, items } => {
                rows.push(row(label.clone(), None, false, false, false));
                add_rows(items, true, rows);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: usize, label: &str, grayed: bool) -> MenuItem {
        MenuItem::Item { id, label: label.to_string(), checked: false, grayed }
    }

    fn menu() -> Menu {
        let items = vec![
            item(1, "Postavička", false),
            MenuItem::Separator,
            MenuItem::Submenu { label: "Nářadí".to_string(), items: vec![item(2, "Wi-Fi", false), item(3, "Ping a trasa", true)] },
            item(4, "Konec", false),
        ];
        Menu::new(4, &items)
    }

    #[test]
    fn the_menu_is_no_bigger_than_its_own_rows() {
        let menu = menu();
        let (w, h) = menu.size();
        assert!((60..600).contains(&w), "width {w}");
        assert!((60..600).contains(&h), "height {h}");
        let tops: Vec<f32> = menu.rows.iter().map(|r| r.top).collect();
        assert!(tops.windows(2).all(|two| two[0] < two[1]), "{tops:?}");
        assert!(menu.rows.last().expect("rows").bottom <= h as f32);
    }

    #[test]
    fn the_row_under_a_point_is_one_that_can_be_chosen() {
        let menu = menu();
        let middle = |row: &MenuRow| (menu.size().0 / 2, ((row.top + row.bottom) / 2.0) as i32);
        let at = |i: usize| menu.row_at(middle(&menu.rows[i]));
        assert_eq!(at(0).and_then(|row| menu.id(row)), Some(1), "the first item");
        assert_eq!(at(1), None, "the separator");
        assert_eq!(at(2), None, "the name of the group");
        assert_eq!(at(3).and_then(|row| menu.id(row)), Some(2), "an item of the group");
        assert_eq!(at(4), None, "a grayed item");
        assert_eq!(at(5).and_then(|row| menu.id(row)), Some(4), "the last item");
        let (w, h) = menu.size();
        for off in [(-1, 4), (w, 4), (4, -1), (4, h)] {
            assert_eq!(menu.row_at(off), None, "{off:?} is off the box");
        }
    }

    #[test]
    fn the_menu_is_drawn_again_only_when_it_has_changed() {
        let mut menu = menu();
        assert!(menu.frame().is_some(), "the first time");
        assert!(menu.frame().is_none(), "nothing has changed");
        menu.set_hover(Some(0));
        let canvas = menu.frame().expect("lit");
        assert_eq!((canvas.width as i32, canvas.height as i32), menu.size());
        assert!(menu.frame().is_none());
    }
}
