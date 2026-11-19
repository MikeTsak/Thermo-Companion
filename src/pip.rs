//! Pip, the mascot: pictures (for the dashboard), stickers (for the printer) and things he says.

pub static HAPPY: &[u8] = include_bytes!("../assets/pip/pip_happy.png");
pub static WINK: &[u8] = include_bytes!("../assets/pip/pip_wink.png");
pub static SLEEPY: &[u8] = include_bytes!("../assets/pip/pip_sleepy.png");
pub static BUSY: &[u8] = include_bytes!("../assets/pip/pip_busy.png");
pub static PARTY: &[u8] = include_bytes!("../assets/pip/pip_party.png");
pub static SAD: &[u8] = include_bytes!("../assets/pip/pip_sad.png");

static STICKER_HAPPY: &[u8] = include_bytes!("../assets/pip/sticker_happy.png");
static STICKER_PARTY: &[u8] = include_bytes!("../assets/pip/sticker_party.png");
static STICKER_SLEEPY: &[u8] = include_bytes!("../assets/pip/sticker_sleepy.png");
static STICKER_BUSY: &[u8] = include_bytes!("../assets/pip/sticker_busy.png");
static STICKER_SAD: &[u8] = include_bytes!("../assets/pip/sticker_sad.png");

pub static ICON: &[u8] = include_bytes!("../assets/icon.ico");
pub static LOGO: &[u8] = include_bytes!("../assets/logo.png");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Happy,
    Wink,
    Sleepy,
    Busy,
    Party,
    Sad,
}

impl Mood {
    pub fn sticker(self) -> &'static [u8] {
        match self {
            Mood::Happy | Mood::Wink => STICKER_HAPPY,
            Mood::Party => STICKER_PARTY,
            Mood::Sleepy => STICKER_SLEEPY,
            Mood::Busy => STICKER_BUSY,
            Mood::Sad => STICKER_SAD,
        }
    }
}

fn pick(list: &[&'static str], slot: u64) -> &'static str {
    list[(slot as usize) % list.len()]
}

/// What Pip says right now. `slot` changes every few seconds so the line rotates.
pub fn say(mood: Mood, hour: u32, slot: u64) -> &'static str {
    const MORNING: &[&str] = &[
        "good morning! paper loaded, coffee not included.",
        "booting up... all systems sleepy but ready.",
        "new day, new slip. what are we printing?",
    ];
    const AFTERNOON: &[&str] = &[
        "afternoon check: have you had water today?",
        "i've been staring at my paper slot. riveting.",
        "psst. stand up and stretch. i'll wait.",
    ];
    const EVENING: &[&str] = &[
        "wrapping up? i can print your end-of-day receipt.",
        "evening! your tasks are looking brave.",
        "ctrl+alt+p anywhere and i'll print what you selected.",
    ];
    const NIGHT: &[&str] = &[
        "it's late... even robots need sleep. zzz.",
        "shhh. printing quietly. brrrt (softly).",
    ];
    const TIPS: &[&str] = &[
        "tip: drag a picture onto the Print page and i'll dither it for the paper.",
        "tip: pomodoro slips make great desk trophies.",
        "tip: i can print your wi-fi as a QR code for guests.",
        "tip: set a morning briefing and wake up to a receipt.",
        "tip: ctrl+alt+p prints whatever text you have selected.",
    ];
    const SAD: &[&str] = &[
        "i can't find my printer... is it plugged in and on?",
        "no printer, no brrrt. check Settings > printer.",
    ];
    const BUSY: &[&str] = &["printing... brrrrt!", "hold on, ink-free magic in progress."];
    const PARTY: &[&str] = &["all done! you're on fire.", "achievement unlocked. i'm printing a medal."];

    match mood {
        Mood::Sad => pick(SAD, slot),
        Mood::Busy => pick(BUSY, slot),
        Mood::Party => pick(PARTY, slot),
        Mood::Wink => pick(TIPS, slot),
        Mood::Sleepy => pick(NIGHT, slot),
        Mood::Happy => match hour {
            5..=11 => pick(MORNING, slot),
            12..=17 => pick(AFTERNOON, slot),
            18..=22 => pick(EVENING, slot),
            _ => pick(NIGHT, slot),
        },
    }
}

/// A line for printed slips (briefing, end of day).
pub fn slip_line(slot: u64) -> &'static str {
    const LINES: &[&str] = &[
        "Small steps still print.",
        "You've got this. - Pip",
        "Done is better than perfect.",
        "Hydrate. Stretch. Ship.",
        "Today's bug is tomorrow's story.",
        "Paper jams are just surprise parties.",
        "Make it work, make it right, make it print.",
    ];
    pick(LINES, slot)
}
