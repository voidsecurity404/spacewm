//! A question with two buttons in the middle of the screen ("Close Steam there and open
//! it here?"). While it is open it takes the keys and the mouse: Enter (or y) picks the
//! highlighted button, Escape (or n) says no, Left/Right/Tab move the highlight, a
//! click picks a button. A click beside the buttons does nothing, so a stray click
//! never decides.

use smithay::{
    input::{keyboard::Keysym, pointer::CursorImageStatus},
    utils::{Logical, Point},
};

use crate::{
    inventory::{draw::Canvas, Image},
    state::SpaceState,
};

/// Layout in logical px.
const WIDTH: i32 = 660;
const PAD: i32 = 20;
const TITLE_H: i32 = 30;
const LINE_H: i32 = 22;
const BUTTON_H: i32 = 38;
const BUTTON_GAP: i32 = 14;

/// What "yes" does.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmAction {
    /// Close the app's copy outside spacewm and open it here.
    /// `cwd`: where it was typed (a terminal), to start it there.
    MoveHere { app: String, cmd: String, pids: Vec<u32>, cwd: Option<std::path::PathBuf> },
}

pub struct Confirm {
    pub title: String,
    pub lines: Vec<String>,
    pub yes: String,
    pub no: String,
    pub action: ConfirmAction,
    /// The highlighted button: true = yes.
    pub on_yes: bool,
    pub image: Option<Image>,
    drawn: Option<(bool, i32)>,
}

pub fn box_size(lines: usize) -> (i32, i32) {
    (WIDTH, PAD + TITLE_H + lines as i32 * LINE_H + PAD + BUTTON_H + PAD)
}

pub fn box_origin(lines: usize, output: (i32, i32)) -> (i32, i32) {
    let (w, h) = box_size(lines);
    ((output.0 - w) / 2, ((output.1 - h) / 2).max(0))
}

/// The two buttons, (x, y, w, h) inside the box: yes on the left, no on the right.
fn buttons(lines: usize) -> [(i32, i32, i32, i32); 2] {
    let (_, h) = box_size(lines);
    let w = (WIDTH - 2 * PAD - BUTTON_GAP) / 2;
    let y = h - PAD - BUTTON_H;
    [(PAD, y, w, BUTTON_H), (PAD + w + BUTTON_GAP, y, w, BUTTON_H)]
}

/// Which button is at `p` (box-relative): Some(true) yes, Some(false) no.
pub fn button_at(lines: usize, p: (f64, f64)) -> Option<bool> {
    let [yes, no] = buttons(lines);
    let inside = |(x, y, w, h): (i32, i32, i32, i32)| {
        p.0 >= x as f64 && p.0 < (x + w) as f64 && p.1 >= y as f64 && p.1 < (y + h) as f64
    };
    if inside(yes) {
        Some(true)
    } else if inside(no) {
        Some(false)
    } else {
        None
    }
}

impl SpaceState {
    pub fn open_confirm(
        &mut self,
        title: String,
        lines: Vec<String>,
        yes: String,
        no: String,
        action: ConfirmAction,
    ) {
        tracing::info!(title, "question shown");
        self.close_menu();
        self.switcher_cancel();
        self.set_dock_cursor(false);
        self.view.controls.clear();
        if let Some(geo) = self.primary_output_geometry() {
            self.pointer_location = (geo.loc + geo.size.downscale(2).to_point()).to_f64();
        }
        self.cursor_status = CursorImageStatus::default_named();
        self.confirm =
            Some(Confirm { title, lines, yes, no, action, on_yes: true, image: None, drawn: None });
        self.view.dirty = true;
        self.queue_redraw();
    }

    /// Close the question; `yes`: do what it asked.
    pub fn answer_confirm(&mut self, yes: bool) {
        let Some(c) = self.confirm.take() else { return };
        tracing::info!(yes, "question answered");
        self.view.dirty = true;
        self.queue_redraw();
        if !yes {
            return;
        }
        match c.action {
            ConfirmAction::MoveHere { app, cmd, pids, cwd } => self.move_here(app, cmd, pids, cwd),
        }
    }

    pub fn confirm_key(&mut self, sym: Keysym) {
        let Some(on_yes) = self.confirm.as_ref().map(|c| c.on_yes) else { return };
        match sym {
            Keysym::Return | Keysym::KP_Enter | Keysym::space => self.answer_confirm(on_yes),
            Keysym::y | Keysym::Y => self.answer_confirm(true),
            Keysym::Escape | Keysym::n | Keysym::N => self.answer_confirm(false),
            Keysym::Left | Keysym::Right | Keysym::Tab | Keysym::ISO_Left_Tab => {
                if let Some(c) = self.confirm.as_mut() {
                    c.on_yes = !c.on_yes;
                }
                self.queue_redraw();
            }
            _ => {}
        }
    }

    fn confirm_local(&self) -> Option<(usize, (f64, f64))> {
        let geo = self.primary_output_geometry()?;
        let c = self.confirm.as_ref()?;
        let o = box_origin(c.lines.len(), (geo.size.w, geo.size.h));
        let local =
            self.pointer_location - geo.loc.to_f64() - Point::from((o.0 as f64, o.1 as f64));
        Some((c.lines.len(), (local.x, local.y)))
    }

    pub fn confirm_pointer(&mut self, pos: Point<f64, Logical>) {
        let Some(geo) = self.primary_output_geometry() else { return };
        let max: Point<f64, Logical> = Point::from((
            (geo.loc.x + geo.size.w) as f64 - 1.0,
            (geo.loc.y + geo.size.h) as f64 - 1.0,
        ));
        self.pointer_location = Point::from((
            pos.x.clamp(geo.loc.x as f64, max.x),
            pos.y.clamp(geo.loc.y as f64, max.y),
        ));
        if let Some((n, p)) = self.confirm_local() {
            if let (Some(b), Some(c)) = (button_at(n, p), self.confirm.as_mut()) {
                c.on_yes = b;
            }
        }
        self.queue_redraw();
    }

    pub fn confirm_click(&mut self) {
        if let Some(b) = self.confirm_local().and_then(|(n, p)| button_at(n, p)) {
            self.answer_confirm(b);
        }
    }

    /// Before each frame: redraw the box if the highlight changed.
    pub fn refresh_confirm(&mut self) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|o| o.current_scale().fractional_scale().ceil() as i32)
            .unwrap_or(1)
            .max(1);
        let Some(c) = self.confirm.as_ref() else { return };
        if c.drawn == Some((c.on_yes, scale)) {
            return;
        }
        let image = draw(c, scale);
        if let Some(c) = self.confirm.as_mut() {
            c.image = image;
            c.drawn = Some((c.on_yes, scale));
        }
    }
}

fn draw(c: &Confirm, scale: i32) -> Option<Image> {
    let s = scale;
    let (w, h) = box_size(c.lines.len());
    let mut canvas = Canvas::new(w * s, h * s);
    canvas.fill(0, 0, w * s, h * s, [0.07, 0.08, 0.10, 0.96]);
    canvas.frame(0, 0, w * s, h * s, 2 * s, [0.95, 0.70, 0.25, 1.0]);
    canvas.text(&c.title, 18.0, [1.0, 0.86, 0.45, 1.0], PAD * s, PAD * s, s);
    for (i, line) in c.lines.iter().enumerate() {
        let y = PAD + TITLE_H + i as i32 * LINE_H;
        canvas.text(line, 14.0, [0.90, 0.92, 0.95, 1.0], PAD * s, y * s, s);
    }
    for ((x, y, bw, bh), (label, yes)) in
        buttons(c.lines.len()).into_iter().zip([(&c.yes, true), (&c.no, false)])
    {
        let lit = c.on_yes == yes;
        let fill = match (lit, yes) {
            (true, true) => [0.20, 0.45, 0.80, 1.0],
            (true, false) => [0.40, 0.42, 0.46, 1.0],
            (false, _) => [0.16, 0.18, 0.21, 1.0],
        };
        canvas.fill(x * s, y * s, bw * s, bh * s, fill);
        canvas.frame(x * s, y * s, bw * s, bh * s, s, [0.45, 0.50, 0.58, 1.0]);
        let key = if yes { "Enter" } else { "Esc" };
        let text = format!("{label}  ({key})");
        let tw = Canvas::text_width(&text, 14.0, s);
        canvas.text(&text, 14.0, [1.0; 4], x * s + (bw * s - tw) / 2, (y + 10) * s, s);
    }
    let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
        &canvas.rgba,
        smithay::backend::allocator::Fourcc::Abgr8888,
        (canvas.width, canvas.height),
        s,
        smithay::utils::Transform::Normal,
        None,
    );
    Some((buffer, (w, h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_hit_test() {
        let [yes, no] = buttons(3);
        assert_eq!(button_at(3, ((yes.0 + 5) as f64, (yes.1 + 5) as f64)), Some(true));
        assert_eq!(button_at(3, ((no.0 + 5) as f64, (no.1 + 5) as f64)), Some(false));
        assert_eq!(button_at(3, (5.0, 5.0)), None, "the title is no button");
        assert!(yes.0 + yes.2 < no.0, "side by side, apart");
        assert!(no.0 + no.2 <= WIDTH - PAD + 1);
    }

    #[test]
    fn renders() {
        let c = Confirm {
            title: "Steam is already open outside spacewm".into(),
            lines: vec!["a".into(), "b".into()],
            yes: "Close there, open here".into(),
            no: "Leave it".into(),
            action: ConfirmAction::MoveHere {
                app: "Steam".into(),
                cmd: "steam".into(),
                pids: vec![],
                cwd: None,
            },
            on_yes: true,
            image: None,
            drawn: None,
        };
        let (_, size) = draw(&c, 1).expect("font");
        assert_eq!(size, box_size(2));
    }
}
