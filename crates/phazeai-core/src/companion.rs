//! PhazeAI Companion buddy system.
//!
//! Deterministic companion generation from a user seed (machine ID or username).
//! Each companion has a species, rarity, eye style, hat, animated sprite frames,
//! stats, and contextual personality messages. Shared by both CLI and IDE.

use std::collections::HashMap;

// ── Rarity ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub fn stars(self) -> &'static str {
        match self {
            Self::Common => "★",
            Self::Uncommon => "★★",
            Self::Rare => "★★★",
            Self::Epic => "★★★★",
            Self::Legendary => "★★★★★",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Common => "Common",
            Self::Uncommon => "Uncommon",
            Self::Rare => "Rare",
            Self::Epic => "Epic",
            Self::Legendary => "Legendary",
        }
    }

    fn weight(self) -> u32 {
        match self {
            Self::Common => 60,
            Self::Uncommon => 25,
            Self::Rare => 10,
            Self::Epic => 4,
            Self::Legendary => 1,
        }
    }

    fn stat_floor(self) -> u32 {
        match self {
            Self::Common => 5,
            Self::Uncommon => 15,
            Self::Rare => 25,
            Self::Epic => 35,
            Self::Legendary => 50,
        }
    }
}

const RARITIES: &[Rarity] = &[
    Rarity::Common,
    Rarity::Uncommon,
    Rarity::Rare,
    Rarity::Epic,
    Rarity::Legendary,
];

// ── Species ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Species {
    Duck,
    Goose,
    Blob,
    Cat,
    Dragon,
    Octopus,
    Owl,
    Penguin,
    Turtle,
    Snail,
    Ghost,
    Axolotl,
    Capybara,
    Cactus,
    Robot,
    Rabbit,
    Mushroom,
    Fox,
}

impl Species {
    pub fn name(self) -> &'static str {
        match self {
            Self::Duck => "duck",
            Self::Goose => "goose",
            Self::Blob => "blob",
            Self::Cat => "cat",
            Self::Dragon => "dragon",
            Self::Octopus => "octopus",
            Self::Owl => "owl",
            Self::Penguin => "penguin",
            Self::Turtle => "turtle",
            Self::Snail => "snail",
            Self::Ghost => "ghost",
            Self::Axolotl => "axolotl",
            Self::Capybara => "capybara",
            Self::Cactus => "cactus",
            Self::Robot => "robot",
            Self::Rabbit => "rabbit",
            Self::Mushroom => "mushroom",
            Self::Fox => "fox",
        }
    }

    /// Render a compact inline face (for status bars, chat bubbles, etc.)
    pub fn face(self, eye: Eye) -> String {
        let e = eye.ch();
        match self {
            Self::Duck | Self::Goose => format!("({e}>"),
            Self::Blob => format!("({e}{e})"),
            Self::Cat => format!("={e}ω{e}="),
            Self::Dragon => format!("<{e}~{e}>"),
            Self::Octopus => format!("~({e}{e})~"),
            Self::Owl => format!("({e})({e})"),
            Self::Penguin => format!("({e}>)"),
            Self::Turtle => format!("[{e}_{e}]"),
            Self::Snail => format!("{e}(@)"),
            Self::Ghost => format!("/{e}{e}\\"),
            Self::Axolotl => format!("}}{e}.{e}{{"),
            Self::Capybara => format!("({e}oo{e})"),
            Self::Cactus | Self::Mushroom => format!("|{e}  {e}|"),
            Self::Robot => format!("[{e}{e}]"),
            Self::Rabbit => format!("({e}..{e})"),
            Self::Fox => format!("({e}v{e})"),
        }
    }
}

const ALL_SPECIES: &[Species] = &[
    Species::Duck,
    Species::Goose,
    Species::Blob,
    Species::Cat,
    Species::Dragon,
    Species::Octopus,
    Species::Owl,
    Species::Penguin,
    Species::Turtle,
    Species::Snail,
    Species::Ghost,
    Species::Axolotl,
    Species::Capybara,
    Species::Cactus,
    Species::Robot,
    Species::Rabbit,
    Species::Mushroom,
    Species::Fox,
];

// ── Eyes & Hats ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eye {
    Dot,
    Star,
    Cross,
    Circle,
    At,
    Degree,
}

impl Eye {
    pub fn ch(self) -> char {
        match self {
            Self::Dot => '·',
            Self::Star => '✦',
            Self::Cross => '×',
            Self::Circle => '◉',
            Self::At => '@',
            Self::Degree => '°',
        }
    }
}

const ALL_EYES: &[Eye] = &[
    Eye::Dot,
    Eye::Star,
    Eye::Cross,
    Eye::Circle,
    Eye::At,
    Eye::Degree,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hat {
    None,
    Crown,
    TopHat,
    Propeller,
    Halo,
    Wizard,
    Beanie,
}

impl Hat {
    pub fn line(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Crown => "   \\^^^/    ",
            Self::TopHat => "   [___]    ",
            Self::Propeller => "    -+-     ",
            Self::Halo => "   (   )    ",
            Self::Wizard => "    /^\\     ",
            Self::Beanie => "   (___)    ",
        }
    }
}

const ALL_HATS: &[Hat] = &[
    Hat::None,
    Hat::Crown,
    Hat::TopHat,
    Hat::Propeller,
    Hat::Halo,
    Hat::Wizard,
    Hat::Beanie,
];

// ── Stats ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatName {
    Debugging,
    Patience,
    Chaos,
    Wisdom,
    Snark,
}

const ALL_STATS: &[StatName] = &[
    StatName::Debugging,
    StatName::Patience,
    StatName::Chaos,
    StatName::Wisdom,
    StatName::Snark,
];

impl StatName {
    pub fn label(self) -> &'static str {
        match self {
            Self::Debugging => "DEBUGGING",
            Self::Patience => "PATIENCE",
            Self::Chaos => "CHAOS",
            Self::Wisdom => "WISDOM",
            Self::Snark => "SNARK",
        }
    }
}

// ── Companion struct ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Companion {
    pub species: Species,
    pub rarity: Rarity,
    pub eye: Eye,
    pub hat: Hat,
    pub shiny: bool,
    pub stats: HashMap<StatName, u32>,
    pub name: String,
}

impl Companion {
    /// Render the sprite for the current animation frame (0, 1, or 2).
    /// Returns 4-5 lines, 12 chars wide.
    pub fn sprite(&self, frame: usize) -> Vec<String> {
        let frames = sprite_frames(self.species);
        let body: Vec<String> = frames[frame % frames.len()]
            .iter()
            .map(|line| line.replace("{E}", &self.eye.ch().to_string()))
            .collect();

        let mut lines = body;

        // Replace empty first line with hat if applicable
        if self.hat != Hat::None {
            if let Some(first) = lines.first() {
                if first.trim().is_empty() {
                    lines[0] = Hat::line(self.hat).to_string();
                }
            }
        }

        // Drop blank hat slot if no hat and all frames have blank line 0
        if self.hat == Hat::None {
            let all_blank = frames.iter().all(|f| f[0].trim().is_empty());
            if all_blank {
                if let Some(first) = lines.first() {
                    if first.trim().is_empty() {
                        lines.remove(0);
                    }
                }
            }
        }

        lines
    }

    /// Compact face string for inline display.
    pub fn face(&self) -> String {
        self.species.face(self.eye)
    }

    /// Stats bar string like "DBG:82 PAT:45 CHA:12 WIS:67 SNK:91"
    pub fn stats_line(&self) -> String {
        ALL_STATS
            .iter()
            .map(|s| {
                let val = self.stats.get(s).copied().unwrap_or(0);
                let short = &s.label()[..3];
                format!("{short}:{val}")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ── Deterministic generation ─────────────────────────────────────────────────

/// Mulberry32 — tiny seeded PRNG.
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6D2B79F5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(1 | t);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        let idx = (self.next() * items.len() as f64) as usize;
        items[idx.min(items.len() - 1)]
    }
}

fn hash_string(s: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h
}

/// Generate a deterministic companion from a seed string (e.g. machine ID, username).
pub fn generate(seed: &str) -> Companion {
    let salt = "phazeai-buddy-2026";
    let hash = hash_string(&format!("{seed}{salt}"));
    let mut rng = Rng::new(hash);

    // Roll rarity
    let total: u32 = RARITIES.iter().map(|r| r.weight()).sum();
    let mut roll = rng.next() * total as f64;
    let mut rarity = Rarity::Common;
    for &r in RARITIES {
        roll -= r.weight() as f64;
        if roll < 0.0 {
            rarity = r;
            break;
        }
    }

    let species = rng.pick(ALL_SPECIES);
    let eye = rng.pick(ALL_EYES);
    let hat = if rarity == Rarity::Common {
        Hat::None
    } else {
        rng.pick(ALL_HATS)
    };
    let shiny = rng.next() < 0.01;

    // Roll stats
    let floor = rarity.stat_floor();
    let peak = rng.pick(ALL_STATS);
    let mut dump = rng.pick(ALL_STATS);
    while dump == peak {
        dump = rng.pick(ALL_STATS);
    }

    let mut stats = HashMap::new();
    for &name in ALL_STATS {
        let val = if name == peak {
            (floor + 50 + (rng.next() * 30.0) as u32).min(100)
        } else if name == dump {
            (floor as i32 - 10 + (rng.next() * 15.0) as i32).max(1) as u32
        } else {
            floor + (rng.next() * 40.0) as u32
        };
        stats.insert(name, val);
    }

    // Generate a name from the seed
    let name = generate_name(&mut rng, species);

    Companion {
        species,
        rarity,
        eye,
        hat,
        shiny,
        stats,
        name,
    }
}

/// Get the user's seed string (machine hostname or fallback).
pub fn user_seed() -> String {
    // Try machine hostname first, then username, then "anon"
    if let Ok(hostname) = std::env::var("HOSTNAME") {
        return hostname;
    }
    if let Ok(user) = std::env::var("USER") {
        return user;
    }
    if let Ok(user) = std::env::var("USERNAME") {
        return user;
    }
    "anon".to_string()
}

fn generate_name(rng: &mut Rng, species: Species) -> String {
    let prefixes: &[&str] = match species {
        Species::Duck | Species::Goose => &["Quack", "Waddle", "Honk", "Feather", "Splash"],
        Species::Blob => &["Bloop", "Goo", "Squish", "Jelly", "Wobble"],
        Species::Cat => &["Whisker", "Purr", "Meow", "Shadow", "Luna"],
        Species::Dragon => &["Blaze", "Ember", "Flame", "Cinder", "Spark"],
        Species::Octopus => &["Ink", "Tentacle", "Squid", "Coral", "Wave"],
        Species::Owl => &["Hoot", "Talon", "Sage", "Noctis", "Feather"],
        Species::Penguin => &["Waddle", "Frost", "Chill", "Flip", "Tux"],
        Species::Turtle => &["Shell", "Slow", "Steady", "Mossy", "Ancient"],
        Species::Snail => &["Trail", "Glide", "Spiral", "Dew", "Slick"],
        Species::Ghost => &["Boo", "Phantom", "Wisp", "Shade", "Misty"],
        Species::Axolotl => &["Axel", "Gill", "Pink", "Frilly", "Lotl"],
        Species::Capybara => &["Capy", "Chill", "Zen", "Mellow", "Buddy"],
        Species::Cactus => &["Spike", "Prick", "Sandy", "Dry", "Bloom"],
        Species::Robot => &["Bolt", "Byte", "Circuit", "Beep", "Gear"],
        Species::Rabbit => &["Hop", "Fluff", "Bounce", "Cotton", "Clover"],
        Species::Mushroom => &["Spore", "Cap", "Fungi", "Truffle", "Morel"],
        Species::Fox => &["Rusty", "Swift", "Sly", "Amber", "Blitz"],
    };

    let suffixes: &[&str] = &[
        "o", "y", "ie", "ster", "ling", "ton", "bug", "paws", "bits", "jr",
    ];

    let prefix = rng.pick(prefixes);
    let suffix = rng.pick(suffixes);
    format!("{prefix}{suffix}")
}

// ── Sprite data (5 lines tall, ~12 wide, {E} = eye placeholder) ─────────────

fn sprite_frames(species: Species) -> &'static [&'static [&'static str]] {
    match species {
        Species::Duck => &[
            &[
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  ._>   ",
                "    `--´    ",
            ],
            &[
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  ._>   ",
                "    `--´~   ",
            ],
            &[
                "            ",
                "    __      ",
                "  <({E} )___  ",
                "   (  .__>  ",
                "    `--´    ",
            ],
        ],
        Species::Goose => &[
            &[
                "            ",
                "     ({E}>    ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
            &[
                "            ",
                "    ({E}>     ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
            &[
                "            ",
                "     ({E}>>   ",
                "     ||     ",
                "   _(__)_   ",
                "    ^^^^    ",
            ],
        ],
        Species::Blob => &[
            &[
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (      )  ",
                "   `----´   ",
            ],
            &[
                "            ",
                "  .------.  ",
                " (  {E}  {E}  ) ",
                " (        ) ",
                "  `------´  ",
            ],
            &[
                "            ",
                "    .--.    ",
                "   ({E}  {E})   ",
                "   (    )   ",
                "    `--´    ",
            ],
        ],
        Species::Cat => &[
            &[
                "            ",
                "   /\\_/\\    ",
                "  ( {E}   {E})  ",
                "  (  ω  )   ",
                "  (\")_(\")   ",
            ],
            &[
                "            ",
                "   /\\_/\\    ",
                "  ( {E}   {E})  ",
                "  (  ω  )   ",
                "  (\")_(\")~  ",
            ],
            &[
                "            ",
                "   /\\-/\\    ",
                "  ( {E}   {E})  ",
                "  (  ω  )   ",
                "  (\")_(\")   ",
            ],
        ],
        Species::Dragon => &[
            &[
                "            ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (   ~~   ) ",
                "  `-vvvv-´  ",
            ],
            &[
                "            ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (        ) ",
                "  `-vvvv-´  ",
            ],
            &[
                "   ~    ~   ",
                "  /^\\  /^\\  ",
                " <  {E}  {E}  > ",
                " (   ~~   ) ",
                "  `-vvvv-´  ",
            ],
        ],
        Species::Octopus => &[
            &[
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  /\\/\\/\\/\\  ",
            ],
            &[
                "            ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  \\/\\/\\/\\/  ",
            ],
            &[
                "     o      ",
                "   .----.   ",
                "  ( {E}  {E} )  ",
                "  (______)  ",
                "  /\\/\\/\\/\\  ",
            ],
        ],
        Species::Owl => &[
            &[
                "            ",
                "   /\\  /\\   ",
                "  (({E})({E}))  ",
                "  (  ><  )  ",
                "   `----´   ",
            ],
            &[
                "            ",
                "   /\\  /\\   ",
                "  (({E})({E}))  ",
                "  (  ><  )  ",
                "   .----.   ",
            ],
            &[
                "            ",
                "   /\\  /\\   ",
                "  (({E})(-))  ",
                "  (  ><  )  ",
                "   `----´   ",
            ],
        ],
        Species::Penguin => &[
            &[
                "            ",
                "  .---.     ",
                "  ({E}>{E})     ",
                " /(   )\\    ",
                "  `---´     ",
            ],
            &[
                "            ",
                "  .---.     ",
                "  ({E}>{E})     ",
                " |(   )|    ",
                "  `---´     ",
            ],
            &[
                "  .---.     ",
                "  ({E}>{E})     ",
                " /(   )\\    ",
                "  `---´     ",
                "   ~ ~      ",
            ],
        ],
        Species::Turtle => &[
            &[
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[______]\\ ",
                "  ``    ``  ",
            ],
            &[
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[______]\\ ",
                "   ``  ``   ",
            ],
            &[
                "            ",
                "   _,--._   ",
                "  ( {E}  {E} )  ",
                " /[======]\\ ",
                "  ``    ``  ",
            ],
        ],
        Species::Snail => &[
            &[
                "            ",
                " {E}    .--.  ",
                "  \\  ( @ )  ",
                "   \\_`--´   ",
                "  ~~~~~~~   ",
            ],
            &[
                "            ",
                "  {E}   .--.  ",
                "  |  ( @ )  ",
                "   \\_`--´   ",
                "  ~~~~~~~   ",
            ],
            &[
                "            ",
                " {E}    .--.  ",
                "  \\  ( @  ) ",
                "   \\_`--´   ",
                "   ~~~~~~   ",
            ],
        ],
        Species::Ghost => &[
            &[
                "            ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  ~`~``~`~  ",
            ],
            &[
                "            ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  `~`~~`~`  ",
            ],
            &[
                "    ~  ~    ",
                "   .----.   ",
                "  / {E}  {E} \\  ",
                "  |      |  ",
                "  ~~`~~`~~  ",
            ],
        ],
        Species::Axolotl => &[
            &[
                "            ",
                "}~(______)~{",
                "}~({E} .. {E})~{",
                "  ( .--. )  ",
                "  (_/  \\_)  ",
            ],
            &[
                "            ",
                "~}(______){~",
                "~}({E} .. {E}){~",
                "  ( .--. )  ",
                "  (_/  \\_)  ",
            ],
            &[
                "            ",
                "}~(______)~{",
                "}~({E} .. {E})~{",
                "  (  --  )  ",
                "  ~_/  \\_~  ",
            ],
        ],
        Species::Capybara => &[
            &[
                "            ",
                "  n______n  ",
                " ( {E}    {E} ) ",
                " (   oo   ) ",
                "  `------´  ",
            ],
            &[
                "            ",
                "  n______n  ",
                " ( {E}    {E} ) ",
                " (   Oo   ) ",
                "  `------´  ",
            ],
            &[
                "    ~  ~    ",
                "  u______n  ",
                " ( {E}    {E} ) ",
                " (   oo   ) ",
                "  `------´  ",
            ],
        ],
        Species::Cactus => &[
            &[
                "            ",
                " n  ____  n ",
                " | |{E}  {E}| | ",
                " |_|    |_| ",
                "   |    |   ",
            ],
            &[
                "            ",
                "    ____    ",
                " n |{E}  {E}| n ",
                " |_|    |_| ",
                "   |    |   ",
            ],
            &[
                " n        n ",
                " |  ____  | ",
                " | |{E}  {E}| | ",
                " |_|    |_| ",
                "   |    |   ",
            ],
        ],
        Species::Robot => &[
            &[
                "            ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ ==== ]  ",
                "  `------´  ",
            ],
            &[
                "            ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ -==- ]  ",
                "  `------´  ",
            ],
            &[
                "     *      ",
                "   .[||].   ",
                "  [ {E}  {E} ]  ",
                "  [ ==== ]  ",
                "  `------´  ",
            ],
        ],
        Species::Rabbit => &[
            &[
                "            ",
                "   (\\__/)   ",
                "  ( {E}  {E} )  ",
                " =(  ..  )= ",
                "  (\")__(\")",
            ],
            &[
                "            ",
                "   (|__/)   ",
                "  ( {E}  {E} )  ",
                " =(  ..  )= ",
                "  (\")__(\")",
            ],
            &[
                "            ",
                "   (\\__/)   ",
                "  ( {E}  {E} )  ",
                " =( .  . )= ",
                "  (\")__(\")",
            ],
        ],
        Species::Mushroom => &[
            &[
                "            ",
                " .-o-OO-o-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
            &[
                "            ",
                " .-O-oo-O-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
            &[
                "   . o  .   ",
                " .-o-OO-o-. ",
                "(__________)",
                "   |{E}  {E}|   ",
                "   |____|   ",
            ],
        ],
        Species::Fox => &[
            &[
                "            ",
                "  /\\    /\\  ",
                " ( {E}    {E} ) ",
                " (   vv   ) ",
                "  `------´  ",
            ],
            &[
                "            ",
                "  /\\    /|  ",
                " ( {E}    {E} ) ",
                " (   vv   ) ",
                "  `------´  ",
            ],
            &[
                "            ",
                "  /\\    /\\  ",
                " ( {E}    {E} ) ",
                " (   vv   ) ",
                "  `------´~ ",
            ],
        ],
    }
}

// ── Contextual messages ──────────────────────────────────────────────────────

pub const IDLE_MESSAGES: &[&str] = &[
    "what are we building today?",
    "i believe in you",
    "type something, i dare you",
    "another day, another diff",
    "let's ship it",
    "ready when you are",
    "got coffee? you'll need it",
    "the codebase isn't gonna fix itself",
    "*stretches* ok let's go",
    "tabs or spaces? just kidding",
    "may your builds be green",
    "bugs fear us",
    "zero warnings, zero mercy",
    "i'm watching... no pressure",
    "git push --force? bold move",
    "semicolons are optional* (*not really)",
];

pub const THINKING_MESSAGES: &[&str] = &[
    "hmm let me think...",
    "working on it...",
    "crunching tokens...",
    "reading your code rn...",
    "oh this is interesting...",
    "one sec...",
    "hold on, cooking...",
    "the AI is doing AI things",
    "neurons firing...",
    "beep boop beep",
    "trust the process",
    "*intense staring at code*",
];

pub const TOOL_MESSAGES: &[&str] = &[
    "ooh tools!",
    "running things...",
    "hope that works lol",
    "executing... fingers crossed",
    "*bites nails*",
    "tools go brrr",
    "deploying chaos responsibly",
    "watch this...",
];

pub const SUCCESS_MESSAGES: &[&str] = &[
    "nailed it!",
    "we're so back",
    "shipped!",
    "that was clean",
    "ezpz",
    "*chef's kiss*",
    "another one bites the dust",
    "you're welcome",
    "too easy",
    "W",
];

pub const ERROR_MESSAGES: &[&str] = &[
    "oof",
    "that's rough buddy",
    "it's not a bug, it's a feature... wait no",
    "have you tried turning it off and on again?",
    "skill issue (jk)",
    "F",
    "rip",
    "well that happened",
    "errors are just learning opportunities",
    "it's fine, everything is fine",
];

pub const APPROVAL_MESSAGES: &[&str] = &[
    "your call boss",
    "approve or deny, no pressure",
    "whatcha think?",
    "i'd approve it but that's just me",
    "the ball is in your court",
    "waiting on you chief",
];

pub const GREETING_MESSAGES: &[&str] = &[
    "hey! ready to code?",
    "welcome back!",
    "let's build something cool",
    "oh hey, you're here!",
    "booting up... just kidding, i'm always ready",
    "the IDE is alive!",
];

/// Pick a message deterministically from a pool based on a tick counter.
pub fn pick_message<'a>(pool: &'a [&'a str], tick: u64) -> &'a str {
    pool[(tick as usize) % pool.len()]
}
