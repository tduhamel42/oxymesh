//! Status pages for a small screen and one button (`docs/spec/ui.md` §12): the Heltec V4's
//! 128×64 OLED. Like the C++ firmware on the same board (`examples/companion_radio/ui-new/
//! UITask.cpp`): a click shows the next page, a double click the previous one, a long press acts
//! on the page; the phone does the rest. Text in the 6×10 font: 21 columns, a header line and 5
//! lines of content. While a phone pairs, its PIN covers the page.
//!
//! The page is built as text ([`Screen`]), which the tests read, then drawn into the
//! [`Oled`] frame the firmware sends to the panel.

use alloc::format;
use alloc::string::String;

use embedded_graphics::Drawable;
use embedded_graphics::Pixel;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::{DrawTarget, OriginDimensions, Point, Primitive, Size};
use embedded_graphics::primitives::{Line, PrimitiveStyle};
use embedded_graphics::text::Text;
use mc_node::history::Peer;

use crate::act::{Intent, Outcome};
use crate::app::{BleState, Ctx};
use crate::clock;
use crate::input::{Button, Gesture};

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 64;
/// Characters per line, lines per page (header included).
pub const COLS: usize = 21;
pub const ROWS: usize = 6;
/// How long an acknowledgement ("Advert sent") stays, ms.
const NOTE_MS: u64 = 2500;

/// The OLED's image: 1 bit per pixel, rows of 16 bytes, MSB first, 1 = lit.
#[derive(Clone, PartialEq, Eq)]
pub struct Oled {
    pub bits: [u8; (WIDTH * HEIGHT / 8) as usize],
}

impl core::fmt::Debug for Oled {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let lit = self.bits.iter().map(|b| b.count_ones()).sum::<u32>();
        write!(f, "Oled({lit} lit pixels)")
    }
}

impl Oled {
    pub const fn new() -> Self {
        Self {
            bits: [0; (WIDTH * HEIGHT / 8) as usize],
        }
    }

    pub fn clear(&mut self) {
        self.bits.fill(0);
    }

    /// Is the pixel at (x, y) lit?
    pub fn lit(&self, x: u32, y: u32) -> bool {
        if x >= WIDTH || y >= HEIGHT {
            return false;
        }
        let i = (y * (WIDTH / 8) + x / 8) as usize;
        self.bits
            .get(i)
            .is_some_and(|b| b & (0x80u8 >> (x % 8)) != 0)
    }
}

impl Default for Oled {
    fn default() -> Self {
        Self::new()
    }
}

impl OriginDimensions for Oled {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Oled {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            let (Ok(x), Ok(y)) = (u32::try_from(p.x), u32::try_from(p.y)) else {
                continue;
            };
            if x >= WIDTH || y >= HEIGHT {
                continue;
            }
            let i = (y * (WIDTH / 8) + x / 8) as usize;
            let m = 0x80u8 >> (x % 8);
            if let Some(b) = self.bits.get_mut(i) {
                if c == BinaryColor::On {
                    *b |= m;
                } else {
                    *b &= !m;
                }
            }
        }
        Ok(())
    }
}

/// The pages, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Messages,
    Heard,
    Radio,
    Advert,
    Off,
}

const PAGES: [Page; 6] = [
    Page::Home,
    Page::Messages,
    Page::Heard,
    Page::Radio,
    Page::Advert,
    Page::Off,
];

/// A page as text: the header, then the content lines; `big` covers the content in the large
/// font (the pairing PIN).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub lines: [String; ROWS],
    pub big: Option<String>,
}

#[derive(Debug, Default)]
pub struct Status {
    page: usize,
    note: Option<(String, u64)>,
}

impl Status {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn page(&self) -> Page {
        PAGES.get(self.page).copied().unwrap_or(Page::Home)
    }

    /// One gesture of the board's button (reported as button A).
    pub fn gesture(&mut self, g: Gesture, c: &Ctx<'_>) -> Option<Intent> {
        match g {
            Gesture::Click(Button::A) => {
                self.page = (self.page + 1) % PAGES.len();
                self.note = None;
                None
            }
            Gesture::Double(Button::A) => {
                self.page = (self.page + PAGES.len() - 1) % PAGES.len();
                self.note = None;
                None
            }
            Gesture::Long(Button::A) | Gesture::HeldEnd(Button::A) => self.act(c),
            _ => None,
        }
    }

    fn act(&mut self, c: &Ctx<'_>) -> Option<Intent> {
        match self.page() {
            Page::Advert => Some(Intent::SendAdvert { flood: true }),
            Page::Off => Some(Intent::Hibernate),
            Page::Messages => newest_unread(c).map(Intent::MarkRead),
            Page::Home | Page::Heard | Page::Radio => None,
        }
    }

    /// What became of an intent from this screen.
    pub fn done(&mut self, i: &Intent, o: &Outcome, now_ms: u64) {
        let text = match (i, o) {
            (Intent::SendAdvert { .. }, Outcome::Ok) => "Advert sent",
            (Intent::MarkRead(_), Outcome::Ok) => "Marked read",
            (_, Outcome::Failed(_)) => "Failed",
            _ => return,
        };
        self.note = Some((String::from(text), now_ms + NOTE_MS));
    }

    /// Is something on screen going to change by itself (a note expiring)?
    pub fn tick(&mut self, now_ms: u64) -> bool {
        if self
            .note
            .as_ref()
            .is_some_and(|(_, until)| now_ms >= *until)
        {
            self.note = None;
            return true;
        }
        false
    }

    /// The page as text.
    pub fn screen(&self, c: &Ctx<'_>) -> Screen {
        let mut lines: [String; ROWS] = Default::default();
        lines[0] = header(c, self.page);
        let body = match self.page() {
            Page::Home => home(c),
            Page::Messages => messages(c),
            Page::Heard => heard(c),
            Page::Radio => radio(c),
            Page::Advert => [
                String::from("Advert"),
                String::from("Long press: send"),
                String::from("one, flooded."),
                String::new(),
                String::new(),
            ],
            Page::Off => [
                String::from("Power off"),
                String::from("Long press: off."),
                String::from("Press to wake."),
                String::new(),
                String::new(),
            ],
        };
        for (l, b) in lines.iter_mut().skip(1).zip(body) {
            *l = fit(&b);
        }
        if let Some((n, _)) = &self.note {
            lines[ROWS - 1] = fit(&format!("> {n}"));
        }
        let big = (c.ble.state == BleState::Pairing).then(|| format!("{:06}", c.ble.pin));
        Screen { lines, big }
    }

    /// Draws the page into the OLED frame.
    pub fn draw(&self, f: &mut Oled, c: &Ctx<'_>) {
        render(&self.screen(c), f);
    }
}

/// Draws a page: the header, a rule, the content; the pairing PIN in the large font instead of
/// the content.
pub fn render(s: &Screen, f: &mut Oled) {
    f.clear();
    let small = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::On);
    let _ = Text::new(&s.lines[0], Point::new(0, 8), small).draw(f);
    let _ = Line::new(Point::new(0, 11), Point::new(WIDTH as i32 - 1, 11))
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
        .draw(f);
    if let Some(b) = &s.big {
        let _ = Text::new("Pairing PIN:", Point::new(0, 24), small).draw(f);
        let x = (WIDTH as i32 - 10 * b.len() as i32) / 2;
        let _ = Text::new(b, Point::new(x.max(0), 50), big).draw(f);
        return;
    }
    for (i, l) in s.lines.iter().enumerate().skip(1) {
        let y = 21 + 10 * (i as i32 - 1);
        let _ = Text::new(l, Point::new(0, y), small).draw(f);
    }
}

/// Cut to the line's width (characters, not bytes).
fn fit(s: &str) -> String {
    s.chars().take(COLS).collect()
}

/// `name … 87%`, with the page number between: `Morgan     2/6   87%`.
fn header(c: &Ctx<'_>, page: usize) -> String {
    let name = name_of(&c.dev.node.prefs.name);
    let battery = match c.battery_percent {
        Some(p) if c.charging => format!("+{p}%"),
        Some(p) => format!("{p}%"),
        None if c.charging => String::from("USB"),
        None => String::new(),
    };
    let right = format!("{}/{} {battery}", page + 1, PAGES.len());
    let room = COLS.saturating_sub(right.chars().count() + 1);
    let name: String = name.chars().take(room).collect();
    let pad = COLS.saturating_sub(name.chars().count() + right.chars().count());
    format!("{name}{}{right}", " ".repeat(pad))
}

fn home(c: &Ctx<'_>) -> [String; 5] {
    let t = clock::local(c.unix, c.tz_offset_min);
    let unread = c.dev.unread_alerting();
    let ble = match c.ble.state {
        BleState::Off => String::from("Bluetooth off"),
        BleState::Advertising => String::from("Bluetooth: no phone"),
        BleState::Pairing => String::from("Bluetooth: pairing"),
        BleState::Connected => String::from("Phone connected"),
    };
    [
        format!("{}  {}", clock::hhmm(t), short_date(t)),
        if unread == 0 {
            String::from("No new messages")
        } else {
            format!("{unread} unread")
        },
        ble,
        format!("Noise {} dBm", c.dev.noise_floor),
        String::new(),
    ]
}

fn short_date(t: Option<clock::DateTime>) -> String {
    match t {
        Some(t) => format!("{}/{:02}", t.day, t.month),
        None => String::new(),
    }
}

/// The newest messages, newest first: `*Bob: coffee at 4?` (`*` unread, `>` sent).
fn messages(c: &Ctx<'_>) -> [String; 5] {
    let mut out: [String; 5] = Default::default();
    out[0] = String::from("Messages");
    let h = c.dev.history();
    let mut n = 0;
    for m in h.iter().collect::<alloc::vec::Vec<_>>().into_iter().rev() {
        let Some(slot) = out.get_mut(1 + n) else {
            break;
        };
        let mark = if m.outgoing {
            '>'
        } else if m.is_unread() {
            '*'
        } else {
            ' '
        };
        let text = String::from_utf8_lossy(&m.text);
        *slot = format!("{mark}{}: {text}", peer_name(c, m.peer));
        n += 1;
    }
    if n == 0 {
        out[1] = String::from("None yet.");
    }
    out
}

/// The conversation of the newest unread message.
fn newest_unread(c: &Ctx<'_>) -> Option<Peer> {
    c.dev
        .history()
        .iter()
        .filter(|m| m.is_unread())
        .max_by_key(|m| m.seq)
        .map(|m| m.peer)
}

/// Nodes heard lately: name, hops, how long ago.
fn heard(c: &Ctx<'_>) -> [String; 5] {
    let mut out: [String; 5] = Default::default();
    out[0] = format!("Heard ({})", c.dev.heard().len());
    let mut nodes: alloc::vec::Vec<_> = c.dev.heard().sorted().into_iter().collect();
    nodes.sort_unstable_by_key(|n| core::cmp::Reverse(n.at));
    for (slot, n) in out.iter_mut().skip(1).zip(nodes) {
        let age = ago(c.unix.saturating_sub(n.at));
        let name = name_of(&n.name);
        let name: String = name.chars().take(12).collect();
        *slot = format!("{name:<12} {}h {age:>4}", n.hops);
    }
    if c.dev.heard().is_empty() {
        out[1] = String::from("Nobody yet.");
    }
    out
}

fn ago(s: u32) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

fn radio(c: &Ctx<'_>) -> [String; 5] {
    let r = &c.dev.node.prefs.radio;
    let bw = if r.bw_hz.is_multiple_of(1000) {
        format!("{}", r.bw_hz / 1000)
    } else {
        format!("{}.{}", r.bw_hz / 1000, r.bw_hz % 1000 / 100)
    };
    [
        String::from("Radio"),
        format!("{}.{:03} MHz", r.freq_khz / 1000, r.freq_khz % 1000),
        format!("SF{} BW{bw} CR4/{}", r.sf, r.cr),
        format!("TX {} dBm", r.tx_power_dbm),
        format!("Noise {} dBm", c.dev.noise_floor),
    ]
}

fn name_of(b: &[u8]) -> String {
    String::from_utf8_lossy(mc_node::contacts::name_str(b)).into_owned()
}

fn peer_name(c: &Ctx<'_>, peer: Peer) -> String {
    let node = &c.dev.node;
    let n = match peer {
        Peer::Contact(p) => node
            .contacts
            .find_prefix(&p)
            .and_then(|s| node.contacts.get(s))
            .map(|c| name_of(&c.name)),
        Peer::Channel(i) => node
            .channels
            .0
            .get(usize::from(i))
            .map(|c| name_of(&c.name)),
    };
    n.filter(|n| !n.is_empty()).unwrap_or_else(|| match peer {
        Peer::Contact(p) => format!("{}", mc_core::Hex(p.get(..4).unwrap_or(&p))),
        Peer::Channel(i) => format!("Channel {i}"),
    })
}
