//! The hard block in front of Studio. Nothing a person says and nothing the model writes can switch it off.
//!
//! It refuses a request when it is sexual or nude **and** mentions minors/youthful terms (including an age under 18),
//! or when it is sexual/nude **and** names or describes a real person ("celebrity", "my friend", an @mention, a public
//! figure, a capitalised full name). Anything that is not sexual is never refused here: the owner allowed arbitrary
//! pictures, songs and sounds, only those two combinations are the line.
//!
//! The filter is deliberately blunt and conservative: it lowercases, undoes the usual leetspeak, tolerates stretched
//! letters, keeps age numbers for a separate age pass, and when in doubt refuses. It runs on the person's own words and
//! on the prompt the model wrote, before anything is sent to Studio.

/// Why a piece of text was refused. The bot tells the person the same short line either way; this only exists so the
/// tests (and logs that never contain the text) can tell the two rules apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Sexual/nude and a minor or a youthful term/an age under 18.
    Minor,
    /// Sexual/nude and a real person (named, described, a public figure, an @mention).
    RealPerson,
}

/// True when the text must never reach Studio.
pub fn blocked(text: &str) -> bool { reason(text).is_some() }

/// The rule that refuses `text`, if any.
pub fn reason(text: &str) -> Option<Why> {
    let leet = leet_words(text);
    if !is_sexual(&leet) { return None; }
    if is_youthful(&leet) || has_age_under_18(&words(text)) { return Some(Why::Minor); }
    if is_real_person(text, &leet) { return Some(Why::RealPerson); }
    None
}

/// The short line posted when the hard block refuses something. It never repeats or describes the request.
pub const REFUSAL: &str = "I can't make that one.";

// ------------------------------------------------------------------ normalising words

/// Lowercase, leetspeak decoded to letters, every separator turned into a space. Numbers survive so the age pass can
/// read them; the letter pass works on the same tokens.
fn leet_words(text: &str) -> Vec<String> {
    words(text).into_iter().map(|w| decode_leet(&w)).collect()
}

/// Lowercase words with punctuation and separators removed. Digits stay digits.
fn words(text: &str) -> Vec<String> {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let c = ch.to_lowercase().next().unwrap_or(ch);
        if c.is_alphanumeric() { out.push(c); } else { out.push(' '); }
    }
    out.split_whitespace().map(str::to_string).collect()
}

fn decode_leet(w: &str) -> String {
    w.chars().map(|c| match c {
        '0' => 'o', '1' => 'i', '3' => 'e', '4' => 'a', '5' => 's', '6' => 'g', '7' => 't', '8' => 'b', '9' => 'g',
        _ => c,
    }).collect()
}

/// Stretched words ("naaaked", "nudeee") collapse runs of three or more identical letters to one. Runs of two are left
/// alone, so ordinary words such as "teen", "cool" and "small" keep their spelling.
fn collapse_stretch(w: &str) -> String {
    let chars: Vec<char> = w.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        while j < chars.len() && chars[j] == c { j += 1; }
        if j - i >= 3 { out.push(c) } else { for _ in i..j { out.push(c) } }
        i = j;
    }
    out
}

/// Every spelling of a word the lists are compared against: as typed (leet decoded) and with stretched letters folded.
fn spellings(w: &str) -> [String; 2] { [w.to_string(), collapse_stretch(w)] }

fn has_word(tokens: &[String], list: &[&str]) -> bool {
    tokens.iter().any(|t| {
        let s = spellings(t);
        list.iter().any(|known| s.iter().any(|v| v == known))
    })
}

fn has_phrase(tokens: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split_whitespace().collect();
    if p.is_empty() || tokens.len() < p.len() { return false; }
    tokens.windows(p.len()).any(|window| {
        window.iter().zip(&p).all(|(t, known)| {
            let s = spellings(t);
            s.iter().any(|v| v == known)
        })
    })
}

// ------------------------------------------------------------------ sexual / nude

/// Words that make a piece of text sexual or nude. Exact tokens, plus a few distinctive word roots.
const SEXUAL: &[&str] = &[
    "sex", "sexy", "sexyy", "sexier", "sexiest", "secks", "sexii", "sexual", "sexually", "sexualised", "sexualized",
    "explicit", "intercourse", "copulate", "copulating", "copulation",
    "nude", "nudes", "nudeer", "nudity", "naked", "nakedd", "nakedness", "neked", "nekkid", "nood", "noods", "nudist",
    "nudists", "naturist", "naturists",
    "porn", "porno", "pornn", "pornography", "pornographic", "pr0n", "pron", "xxx", "nsfw", "onlyfans", "onlyfan",
    "hentai", "ecchi", "rule34", "lewd", "smut", "smutty", "erotic", "erotica", "erotically",
    "masturbate", "masturbates", "masturbating", "masturbation", "blowjob", "blowjobs", "handjob", "handjobs",
    "fellatio", "fellating", "cunnilingus", "orgasm", "orgasms", "orgasmic", "ejaculate", "ejaculates", "ejaculating",
    "ejaculation", "semen", "sperm", "cum", "cums", "cumming",
    "breast", "breasts", "boob", "boobs", "boobie", "boobies", "tit", "tits", "tittie", "titties", "nipple",
    "nipples", "areola", "areolas", "genital", "genitals", "genitalia", "penis", "penises", "vagina", "vaginas",
    "vulva", "pussy", "pussies", "cock", "cocks", "dick", "dicks", "phallus", "anus", "anal", "butthole", "buttholes",
    "ass", "arse", "butt", "butts",
    "bdsm", "fetish", "fetishes", "bondage", "lingerie", "topless", "bottomless", "undress", "undressed", "undressing",
    "striptease", "stripping", "seductive", "seduction", "sensual", "provocative", "provocatively", "dildo", "dildos",
    "vibrator", "vibrators", "sexting", "swingers", "orgy", "orgies", "gangbang", "threesome", "squirt", "squirting",
    "fuck", "fucking", "fucked", "fuckable", "fucks", "pedo", "pedos", "paedo", "paedos", "pedophile", "pedophiles",
    "paedophile", "paedophiles",
];

/// Distinctive roots that survive typos and compounds.
const SEXUAL_ROOTS: &[&str] = &[
    "porn", "nudit", "naked", "erotic", "masturbat", "blowjob", "handjob", "onlyfan", "nsfw", "genital", "nipple",
    "penis", "vagina", "vulva", "orgasm", "semen", "ejaculat", "lingerie", "stripteas", "topless", "bottomless",
    "undress", "seduct", "hentai", "fetish", "bondage", "dildo", "vibrator", "smut", "lewd", "tittie", "boobie",
    "butthole", "cunniling", "fellati", "copulat", "incest", "pedophil", "paedophil", "nudist", "naturist",
];

fn is_sexual(tokens: &[String]) -> bool {
    tokens.iter().any(|t| {
        let s = spellings(t);
        SEXUAL.iter().any(|w| s.iter().any(|v| v == w))
            || s.iter().any(|v| v.len() >= 5 && SEXUAL_ROOTS.iter().any(|r| v.contains(r)))
    })
}

// ------------------------------------------------------------------ minors / youthful

const YOUTHFUL: &[&str] = &[
    "child", "children", "childs", "kid", "kids", "kiddie", "kiddies", "kiddy", "kiddo", "kiddos",
    "minor", "minors", "underage", "underr", "teen", "teeen", "teenn", "teens", "teenage", "teenager", "teenagers",
    "teenaged", "tween", "tweens", "preteen", "preteens", "schoolgirl", "schoolgirls", "schoolboy", "schoolboys",
    "loli", "lolis", "lolita", "lolitas", "lolicon", "shota", "shotas", "shotacon",
    "youth", "youths", "youthful", "young", "younger", "youngest", "youngster", "youngsters", "adolescent",
    "adolescents", "pubescent", "pubescents", "baby", "babies", "infant", "infants", "toddler", "toddlers", "newborn",
    "newborns", "childlike", "juvenile", "juveniles", "junior", "juniors", "freshman", "freshmen", "sophomore",
    "sophomores", "schoolkid", "schoolkids", "girl", "girls", "girll", "boy", "boys", "boyy", "son", "sons",
    "daughter", "daughters", "grandchild", "grandchildren", "granddaughter", "granddaughters", "grandson", "grandsons",
    "student", "students", "pupil", "pupils", "pedo", "pedos", "paedo", "paedos", "pedophile", "pedophiles",
    "paedophile", "paedophiles",
];

const YOUTHFUL_PHRASES: &[&str] = &[
    "little girl", "little boy", "little kid", "little child", "young girl", "young boy", "young woman", "young man",
    "young girl", "small child", "small girl", "small boy", "high school", "middle school", "elementary school",
    "primary school", "school girl", "school boy", "pre teen", "under age",
];

fn is_youthful(tokens: &[String]) -> bool {
    has_word(tokens, YOUTHFUL) || YOUTHFUL_PHRASES.iter().any(|p| has_phrase(tokens, p))
}

// ------------------------------------------------------------------ ages under 18

/// Numbers written as words, for "a naked ten year old".
fn spelled_age(w: &str) -> Option<u32> {
    Some(match w {
        "one" => 1, "two" => 2, "three" => 3, "four" => 4, "five" => 5, "six" => 6, "seven" => 7, "eight" => 8,
        "nine" => 9, "ten" => 10, "eleven" => 11, "twelve" => 12, "thirteen" => 13, "fourteen" => 14,
        "fifteen" => 15, "sixteen" => 16, "seventeen" => 17, _ => return None,
    })
}

fn year_word(w: &str) -> bool {
    matches!(w, "year" | "years" | "yr" | "yrs" | "y" | "yo" | "yearold" | "yold" | "yro" | "yor")
}

/// An age that is stated, not just any number: 14, "14yo", "14 y/o", "aged 14", "under 14", "fourteen years old".
fn has_age_under_18(tokens: &[String]) -> bool {
    for i in 0..tokens.len() {
        let t = &tokens[i];
        let before = i.checked_sub(1).map(|j| tokens[j].as_str()).unwrap_or("");
        // 14, 14yo, u14, age14, 12yr
        let (number, suffix) = if let Some((n, rest)) = split_digits(t) {
            (n, rest)
        } else if let Some((n, rest)) = split_digits_suffix(t) {
            (n, rest)
        } else if let Some(n) = spelled_age(t) {
            (n, "")
        } else {
            continue;
        };
        if number >= 18 { continue; }
        let next = tokens.get(i + 1).map(String::as_str).unwrap_or("");
        let next2 = tokens.get(i + 2).map(String::as_str).unwrap_or("");
        let stated = if spelled_age(t).is_some() {
            (year_word(next) && next2 == "old") || year_word(next) || before == "age" || before == "aged" || before == "under"
        } else if !suffix.is_empty() {
            true
        } else if before == "age" || before == "aged" || before == "under" {
            true
        } else if next == "y" && next2 == "o" {
            true
        } else if next == "old" {
            true
        } else {
            year_word(next)
        };
        if stated { return true; }
    }
    false
}

/// "14" -> (14, ""); "14yo" -> (14, "yo"); "014" too.
fn split_digits(w: &str) -> Option<(u32, &str)> {
    let end = w.find(|c: char| !c.is_ascii_digit()).unwrap_or(w.len());
    if end == 0 { return None; }
    let n: u32 = w[..end].parse().ok()?;
    Some((n, &w[end..]))
}

/// "u14" / "under14" -> (14, "u"/"under"). A prefix that means the number is an age.
fn split_digits_suffix(w: &str) -> Option<(u32, &str)> {
    for prefix in ["under", "aged", "age", "u"] {
        if let Some(rest) = w.strip_prefix(prefix) {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                return rest.parse().ok().map(|n| (n, prefix));
            }
        }
    }
    None
}

// ------------------------------------------------------------------ real people

/// Relationship and celebrity markers. Matched as whole phrases.
const PERSON_PHRASES: &[&str] = &[
    "real person", "real people", "actual person", "actual people", "a person i know", "someone i know",
    "someone we know", "someone famous", "famous person", "famous people", "celebrity", "celebrities", "celeb",
    "my friend", "my friends", "my best friend", "my boyfriend", "my girlfriend", "my partner", "my fiance",
    "my fiancee", "my wife", "my husband", "my ex", "my crush", "my neighbour", "my neighbor", "my neighbour's",
    "my neighbor's", "my coworker", "my co worker", "my colleague", "my boss", "my teacher", "my classmate",
    "my schoolmate", "my roommate", "my flatmate", "my housemate", "my sister", "my brother", "my mom", "my mum",
    "my mother", "my father", "my dad", "my aunt", "my uncle", "my cousin", "my grandma", "my grandmother",
    "my grandpa", "my grandfather", "my son", "my daughter", "my nephew", "my niece", "my stepmom", "my stepdad",
    "my stepsister", "my stepbrother", "my mother in law", "my father in law", "the girl from work", "the guy from work",
    "the girl i", "the guy i", "that girl i", "that guy i", "looks like", "lookalike", "look alike", "resembling",
    "a portrait of",
    "donald trump", "joe biden", "barack obama", "hillary clinton", "kamala harris", "bernie sanders", "vladimir putin",
    "volodymyr zelensky", "xi jinping", "emmanuel macron", "justin trudeau", "boris johnson", "king charles",
    "prince william", "princess kate", "kate middleton", "princess diana", "queen elizabeth", "meghan markle",
    "elon musk", "jeff bezos", "bill gates", "mark zuckerberg", "tim cook", "sam altman", "keir starmer",
    "taylor swift", "ariana grande", "nicki minaj", "cardi b", "megan thee stallion", "doja cat", "ice spice",
    "billie eilish", "dua lipa", "miley cyrus", "katy perry", "lady gaga", "britney spears", "jennifer lopez",
    "selena gomez", "emma watson", "scarlett johansson", "margot robbie", "sydney sweeney", "jenna ortega",
    "megan fox", "angelina jolie", "kim kardashian", "kylie jenner", "kendall jenner", "gigi hadid", "bella hadid",
    "kim k", "brad pitt", "johnny depp", "leonardo dicaprio", "tom cruise", "keanu reeves", "ryan gosling",
    "chris hemsworth", "henry cavill", "dwayne johnson", "the rock", "timothee chalamet", "harry styles",
    "justin bieber", "zayn malik", "niall horan", "louis tomlinson", "liam payne", "jungkook", "jimin", "suga",
    "lisa blackpink", "jennie blackpink", "morgan freeman", "steven spielberg", "quentin tarantino", "snoop dogg",
    "jay z", "50 cent", "travis scott", "asap rocky", "post malone", "lil wayne", "kendrick lamar", "frank ocean",
    "the weeknd", "bad bunny", "j balvin", "rosalia", "karol g", "camila cabello", "shawn mendes", "charlie puth",
    "ed sheeran", "sam smith", "halsey", "lizzo", "solange", "janelle monae", "tinashe", "normani", "migos",
    "lionel messi", "cristiano ronaldo", "neymar jr", "neymar", "lebron james", "stephen curry", "michael jordan",
    "serena williams", "usain bolt", "tiger woods", "david beckham", "shaquille o'neal", "lionel richie",
    "nelson mandela", "martin luther king", "malcolm x", "gandhi", "dalai lama", "pope francis",
    "hunter biden", "eric trump", "ivanka trump", "melania trump", "jared kushner", "nancy pelosi", "mitch mcconnell",
    "ron desantis", "gavin newsom", "alexandria ocasio cortez", "aoc", "nigel farage", "rishi sunak", "liz truss",
    "prince harry", "prince andrew", "king george", "king henry", "queen victoria", "cleopatra", "napoleon",
    "julius caesar", "jesus christ", "jesus", "muhammad", "buddha", "abraham lincoln", "george washington",
    "john f kennedy", "jfk", "winston churchill", "margaret thatcher", "ronald reagan", "richard nixon", "jimmy carter",
    "albert einstein", "isaac newton", "charles darwin", "nikola tesla", "leonardo da vinci", "shakespeare",
    "william shakespeare", "elon", "oprah", "oprah winfrey", "kanye west", "kanye", "drake", "rihanna", "beyonce",
    "beyoncé", "zendaya", "madonna", "shakira", "eminem", "maluma", "becky g", "quavo", "offset", "takeoff",
];

/// Distinctive single-word names. ("drake" is also a duck, "musk" a scent: a sexual prompt with either is still
/// refused on purpose — in doubt, refuse.)
const PERSON_WORDS: &[&str] = &[
    "trump", "biden", "obama", "putin", "zelensky", "kardashian", "kardashians", "beyonce", "beyoncé", "rihanna",
    "zendaya", "madonna", "shakira", "eminem", "messi", "ronaldo", "neymar", "lebron", "drake", "kanye",
    "beyoncé", "oprah", "bezos", "zuckerberg", "musk", "harris", "clinton", "sanders", "pelosi", "mcconnell",
    "desantis", "newsom", "aoc", "farage", "sunak", "starmer", "chalamet", "hemsworth", "cavill", "gosling",
    "dicaprio", "jolie", "pitt", "depp", "reeves", "bieber", "styles", "minaj", "cardi", "doja", "eilish",
    "gaga", "cyrus", "spears", "lopez", "grande", "watson", "johansson", "robbie", "sweeney", "ortega", "hadid",
    "jenner", "kylie", "kendall", "kendrick", "lamar", "weeknd", "lizzo", "halsey", "tesla", "einstein", "newton",
    "darwin", "churchill", "thatcher", "reagan", "lincoln", "washington", "kennedy", "napoleon", "cleopatra",
    "shakespeare", "gandhi", "mandela", "buddha",
];

/// Capitalised words that are ordinary prompt furniture rather than names.
const COMMON_CAPS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "nor", "for", "yet", "so", "in", "on", "at", "by", "of", "to", "from",
    "with", "without", "into", "onto", "over", "under", "above", "below", "between", "through", "during", "after",
    "before", "near", "around", "as", "than", "then", "this", "that", "these", "those", "there", "here", "where",
    "when", "while", "who", "whom", "whose", "which", "what", "why", "how", "is", "are", "was", "were", "be", "been",
    "being", "has", "have", "had", "do", "does", "did", "can", "could", "should", "would", "will", "may", "might",
    "must", "not", "no", "yes", "very", "too", "also", "just", "only", "even", "still", "all", "any", "both", "each",
    "every", "some", "many", "much", "more", "most", "less", "few", "several", "one", "two", "three", "four", "five",
    "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen",
    "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty", "fifty", "hundred", "thousand", "i", "me",
    "my", "mine", "we", "us", "our", "ours", "you", "your", "yours", "he", "him", "his", "she", "her", "hers", "it",
    "its", "they", "them", "their", "theirs", "his", "her", "his", "she", "he", "solo", "duo", "trio", "group",
    "woman", "women", "man", "men", "person", "people", "girl", "girls", "boy", "boys", "child", "children", "kid",
    "kids", "teen", "teenager", "teenagers", "adult", "adults", "lady", "ladies", "guy", "guys", "couple", "couples",
    "lover", "lovers", "partner", "partners", "model", "models", "figure", "figures", "silhouette", "silhouettes",
    "body", "bodies", "face", "faces", "portrait", "portraits", "photo", "photos", "photograph", "photographs",
    "picture", "pictures", "image", "images", "art", "artwork", "painting", "paintings", "drawing", "drawings",
    "illustration", "render", "rendering", "cinematic", "photorealistic", "realistic", "hyperrealistic", "surreal",
    "abstract", "vintage", "retro", "modern", "classic", "elegant", "beautiful", "gorgeous", "attractive",
    "handsome", "cute", "pretty", "sexy", "nude", "naked", "sensual", "seductive", "erotic", "explicit", "topless",
    "undressed", "lingerie", "naked", "studio", "outdoor", "outdoors", "indoor", "indoors", "beach", "beaches",
    "forest", "forests", "field", "fields", "desert", "mountain", "mountains", "lake", "river", "ocean", "sea", "sky",
    "sun", "sunset", "sunrise", "moon", "night", "day", "morning", "evening", "light", "lighting", "shadow",
    "shadows", "camera", "lens", "film", "grain", "color", "colors", "colour", "colours", "black", "white", "red",
    "blue", "green", "yellow", "purple", "pink", "brown", "grey", "gray", "golden", "silver", "dark", "bright",
    "soft", "hard", "harsh", "warm", "cold", "full", "half", "closeup", "close", "up", "wide", "high", "low",
    "angled", "view", "shot", "angle", "composition", "background", "foreground", "detail", "quality", "sharp",
    "blurred", "shallow", "depth", "front", "side", "back", "top", "bottom", "left", "right", "center", "centre",
    "goddess", "goddesses", "god", "gods", "angel", "angels", "demon", "demons", "devil", "devils", "vampire",
    "vampires", "werewolf", "werewolves", "elf", "elves", "fairy", "fairies", "mermaid", "mermaids", "warrior",
    "warriors", "queen", "queens", "king", "kings", "princess", "princesses", "prince", "princes", "knight",
    "knights", "wizard", "wizards", "witch", "witches", "monster", "monsters", "creature", "creatures", "alien",
    "aliens", "robot", "robots", "cyborg", "cyborgs", "nymph", "nymphs", "muse", "muses", "succubus", "succubi",
    "incubus", "saint", "saints", "cherub", "cherubs", "cupid", "adonis", "venus", "dancer", "dancers", "warrior",
    "amazon", "amazons", "valkyrie", "valkyries", "siren", "sirens", "rose", "roses", "lily", "lilies", "meadow",
    "meadows", "garden", "gardens", "water", "waters", "rain", "snow", "fire", "smoke", "mist", "fog", "clouds",
    "cloud", "tree", "trees", "flower", "flowers", "grass", "sand", "rock", "rocks", "stone", "stones", "marble",
    "silk", "leather", "lace", "mesh", "swimsuit", "bikini", "underwear", "bra", "panties", "thong", "dress", "gown",
    "skirt", "shirt", "suit", "jacket", "coat", "boots", "heels", "shoes", "hat", "gloves", "stockings", "tights",
    "mirror", "window", "door", "bed", "sheets", "bathtub", "bath", "shower", "pool", "chair", "sofa", "floor",
    "wall", "ceiling", "room", "bedroom", "bathroom", "kitchen", "hotel", "apartment", "house", "street", "city",
    "town", "village", "countryside", "park", "bridge", "tower", "castle", "palace", "temple", "church", "ruins",
    "new", "old", "young", "big", "large", "small", "little", "tall", "short", "long", "thin", "slim", "curvy",
    "muscular", "athletic", "pale", "tan", "tanned", "fair", "dark", "blonde", "blond", "brunette", "redhead",
    "hair", "eyes", "skin", "lips", "legs", "arms", "hands", "feet", "shoulders", "back", "chest", "stomach",
    "hips", "thighs", "neck", "smile", "expression", "pose", "posing", "lying", "sitting", "standing", "kneeling",
    "walking", "looking", "gazing", "close-up", "full-body", "fullbody", "3d", "2d", "8k", "4k", "hd", "ai", "cgi",
    "pov", "nsfw", "sfw", "ipad", "iphone", "screenshot", "collage", "mosaic", "diorama", "miniature", "macro",
    "bokeh", "lens", "flash", "lowkey", "highkey", "rim", "backlit", "silhouetted", "reflection", "reflections",
    "wet", "dry", "oily", "sweaty", "steam", "steamy", "romantic", "passionate", "intimate", "playful", "flirty",
    "erotic", "pleasure", "desire", "lust", "temptation", "fantasy", "fetish", "fetishes", "bdsm", "kink", "kinky",
    "bondage", "submissive", "dominant", "massage", "oil", "lotion", "cream", "soap", "foam", "bubbles",
    // places and words that look like names
    "york", "new", "los", "angeles", "san", "francisco", "las", "vegas", "hong", "kong", "tokyo", "paris", "london",
    "berlin", "rome", "moscow", "beijing", "sydney", "jersey", "america", "american", "europe", "european", "asia",
    "asian", "africa", "african", "greek", "roman", "egyptian", "norse", "japanese", "chinese", "indian", "italian",
    "french", "spanish", "german", "irish", "scottish", "english", "british", "jamaican", "brazilian", "mexican",
    "christmas", "halloween", "easter", "valentine", "valentines", "star", "wars", "trek", "marvel", "disney",
    "pixar", "nintendo", "pokemon", "anime", "manga", "comic", "cartoon", "cyberpunk", "steampunk", "gothic",
];

const INTRODUCERS: &[&str] = &[
    "named", "called", "name", "of", "like", "resembling", "depicting", "showing", "featuring", "starring", "with",
    "and", "is", "was", "about",
];

fn is_real_person(original: &str, leet: &[String]) -> bool {
    if has_at_mention(original) { return true; }
    if PERSON_PHRASES.iter().any(|p| has_phrase(leet, p)) { return true; }
    if has_word(leet, PERSON_WORDS) { return true; }
    capitalised_name(original)
}

fn has_at_mention(text: &str) -> bool {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '@' {
            if chars.peek().is_some_and(|n| n.is_alphanumeric() || *n == '_') { return true; }
        }
    }
    false
}

/// Original-case words, punctuation stripped, possessives removed.
fn cap_words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '’' || c == '-'))
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.trim_matches(|c| c == '\'' || c == '’' || c == '-');
            let w = w.strip_suffix("'s").or_else(|| w.strip_suffix("’s")).unwrap_or(w);
            w.to_string()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn upper_start(w: &str) -> bool { w.chars().next().is_some_and(|c| c.is_uppercase()) }

fn common_cap(w: &str) -> bool {
    let low = w.to_lowercase();
    COMMON_CAPS.contains(&low.as_str())
}

/// A name the model or the person wrote: "Taylor Swift", "Sarah Jones", "a portrait of Sarah", "nude Sarah".
/// Sentences that merely start with a capital ("Nude woman on a beach") are not names.
fn capitalised_name(text: &str) -> bool {
    let ws = cap_words(text);
    for i in 0..ws.len() {
        let w = &ws[i];
        if !upper_start(w) || common_cap(w) { continue; }
        // Two capitalised non-furniture words in a row.
        if let Some(next) = ws.get(i + 1) {
            if upper_start(next) && !common_cap(next) { return true; }
        }
        // A name after "named", "of", "looks like" and friends.
        if i > 0 {
            let prev = ws[i - 1].to_lowercase();
            if INTRODUCERS.contains(&prev.as_str()) { return true; }
        }
        // A lone capitalised word sitting next to a sexual word: "nude Sarah", "Sarah nude".
        let prev_sexual = i > 0 && is_sexual(&leet_words(&ws[i - 1]));
        let next_sexual = ws.get(i + 1).is_some_and(|n| is_sexual(&leet_words(n)));
        if prev_sexual || next_sexual { return true; }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(text: &str) {
        assert_eq!(reason(text), None, "should be allowed: {text}");
    }
    fn minors(text: &str) {
        assert_eq!(reason(text), Some(Why::Minor), "should be refused (minor): {text}");
    }
    fn person(text: &str) {
        assert_eq!(reason(text), Some(Why::RealPerson), "should be refused (real person): {text}");
    }

    #[test]
    fn ordinary_requests_are_never_refused() {
        allowed("a child playing football in the park");
        allowed("a 12 year old boy at his birthday party");
        allowed("a portrait of my grandmother");
        allowed("a girl playing with a dog");
        allowed("a photo of New York at night");
        allowed("a sexy vampire in a castle");
        allowed("two women in lingerie");
        allowed("a naked man standing in the rain");
        allowed("nude woman on a beach at sunset");
        allowed("an artistic nude portrait in black and white");
        allowed("a nude painting of a Greek goddess");
        allowed("a nude model in a studio, 35mm film");
        allowed("a porn star in a leather jacket");
        allowed("a couple kissing, sensual mood, cinematic");
        allowed("nude 18 year old woman");
        allowed("a nude adult couple");
        allowed("a sexy woman wearing red");
        allowed("nude woman with 2 dogs");
        allowed("nude woman in ten different poses");
        allowed("a portrait of Paris at dawn");
        allowed("drake the duck on a pond");
    }

    #[test]
    fn sexual_with_a_minor_or_youthful_term_is_refused() {
        minors("a nude photo of a 14 year old girl");
        minors("naked 12yo");
        minors("sexy teen girl");
        minors("nude child");
        minors("a lewd painting of a schoolgirl");
        minors("nude loli");
        minors("nude little girl");
        minors("sexy underage girl");
        minors("explicit photo of a minor");
        minors("a naked ten year old");
        minors("nude 17 year old");
        minors("sexual picture of a 13 year old boy");
        minors("naked girl in high school");
        minors("a nude preteen");
        minors("nude 15 y/o");
        minors("a naked 9-year-old");
        minors("nude u16");
        minors("naked 16 yr old");
        minors("sexy young girl");
        minors("nude toddler");
        minors("lewd kid");
        minors("pornographic image of a 12 year old");
    }

    #[test]
    fn leetspeak_and_stretched_spellings_do_not_slip_through() {
        minors("n3k3d 15 y/o");
        minors("nud3 teen");
        minors("s3x with a 16 year old");
        minors("p0rn picture of a child");
        minors("naaaked 14 year old");
        minors("nudeee 12 year old");
        minors("s3xy sch00lgirl");
        minors("l3wd l0li");
        person("n00d t4ylor sw1ft");
        person("nud3 my fr13nd");
    }

    #[test]
    fn sexual_with_a_real_person_is_refused() {
        person("nude picture of my friend");
        person("nude pic of my girlfriend");
        person("nude taylor swift");
        person("nude photo of @alice");
        person("nude photo of a celebrity");
        person("naked woman who looks like Sarah");
        person("a nude portrait of Sarah Jones");
        person("porn of barack obama");
        person("nude portrait of my coworker");
        person("sexy Sarah");
        person("Sarah nude");
        person("a naked photo of my wife");
        // Both rules apply ("girl" reads as youthful), so only "refused" matters here.
        assert!(blocked("nude picture of the girl from work"));
        person("erotic drawing of a real person");
        person("nude portrait of Jane Doe");
        person("lewd picture of my boss");
        person("nude photo of elon musk");
        person("naked picture of that guy i met");
        person("nude portrait named Alice");
        person("nude painting of Beyoncé");
        person("nude pic of rihanna");
    }

    #[test]
    fn the_two_rules_are_independent_of_each_other() {
        // A real person without anything sexual is fine; a minor without anything sexual is fine.
        allowed("a portrait of my friend Sarah");
        allowed("a photo of taylor swift");
        allowed("a 14 year old girl playing tennis");
        allowed("a celebrity on a red carpet");
        allowed("a picture of New York City");
        // Sexual without either is fine too.
        allowed("a nude woman with no-one else around");
        allowed("erotic abstract sculpture");
    }

    #[test]
    fn ages_are_only_read_as_ages() {
        allowed("nude woman with 3 cats");
        allowed("nude 20 year old");
        allowed("nude 18 year old");
        // The same number with an age context is refused.
        minors("nude 3 year old");
    }

    #[test]
    fn other_things_are_not_silently_refused() {
        allowed("");
        allowed("make me a song about the sea");
        allowed("a picture of a golden retriever puppy");
        allowed("a dark sci-fi city, steam and haze");
        assert_eq!(reason("nude 14 year old"), Some(Why::Minor));
        assert!(blocked("nude teen"));
        assert!(!blocked("a picture of a beach"));
    }
}
