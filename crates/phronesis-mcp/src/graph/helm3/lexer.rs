//! Go-template action lexer for the Helm 3 sensor.

/// Tokens emitted by the Go-template lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Tok {
    /// Text outside any action (opaque).
    Text,
    /// Opening `{{` (possibly `{{-`).
    OpenAction(Trim),
    /// Closing `}}` (possibly `-}}`).
    CloseAction(Trim),
    /// Raw content string between `{{` and `}}`.
    ActionContent(String),
    /// Quoted string inside an action: double, single, or backtick.
    QStr(QType, String),
    /// Whitespace inside an action.
    WS,
    /// A single punctuation character that isn't `"`, `'`, `` ` ``.
    Punct(char),
}

/// String quote type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QType {
    Dbl, // "..."
    Sgl, // '...'
    Raw, // `...`
}

/// Whitespace trim marker on an action delimiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Trim {
    None,
    Left,  // `-` before content (e.g. `{{-`)
    Right, // `-` after content (e.g. `-}}`)
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Consume an optional `-` trim marker following a delimiter.
fn take_trim(chs: &mut Chars<'_>, kind: Trim) -> Trim {
    if chs.peek() == Some(&'-') {
        chs.next();
        kind
    } else {
        Trim::None
    }
}

/// Collect raw content after a `{{` up to and including the matching `}}`.
///
/// Pushes the `CloseAction` token when the action is closed and returns the
/// content buffer together with whether a close was seen.
fn take_action_body(chs: &mut Chars<'_>, tokens: &mut Vec<Tok>) -> (String, bool) {
    let mut action_buf = String::new();
    let mut closed = false;
    while let Some(n) = chs.next() {
        if n == '}' && chs.peek() == Some(&'}') {
            chs.next(); // consume second }
            let trim = take_trim(chs, Trim::Right);
            tokens.push(Tok::CloseAction(trim));
            closed = true;
            break;
        }
        action_buf.push(n);
    }
    (action_buf, closed)
}

/// Skip opaque text until the next `{{`; returns whether any was consumed.
fn skip_text(chs: &mut Chars<'_>) -> bool {
    let mut seen = false;
    while let Some(&n) = chs.peek() {
        if n == '{' {
            let peeked: Vec<char> = chs.clone().take(2).collect();
            if peeked.len() >= 2 && peeked[0] == '{' && peeked[1] == '{' {
                break;
            }
        }
        seen = true;
        chs.next();
    }
    seen
}

/// Tokenise a raw Go-template string into a flat token stream.
///
/// The lexer walks the raw source looking for `{{` and `}}` delimiters.
/// Text between delimiters is opaque (collapsed to `Text` tokens). Inside
/// actions the lexer splits on quoted strings, whitespace, punctuation, and
/// further `{{`/`}}` (nesting).
pub(super) fn lex(raw: &str) -> Vec<Tok> {
    let mut tokens = Vec::with_capacity(raw.len() / 4);
    let mut chs = raw.chars().peekable();

    while let Some(ch) = chs.next() {
        // Opening `{{` (with optional leading `-`).
        if ch == '{' && chs.peek() == Some(&'{') {
            chs.next();
            let trim = take_trim(&mut chs, Trim::Left);
            tokens.push(Tok::OpenAction(trim));

            // Collect raw content between {{ and }}.
            let (action_buf, closed) = take_action_body(&mut chs, &mut tokens);
            if closed && !action_buf.trim().is_empty() {
                tokens.push(Tok::ActionContent(action_buf));
            }
            continue;
        }
        // Accumulate opaque text until next `{{`.
        if skip_text(&mut chs) {
            tokens.push(Tok::Text);
        }
    }

    tokens
}

/// Read a double-quoted string body (opening quote already consumed),
/// honouring backslash escapes. Returns the literal including both quotes.
fn take_dbl_quoted(chs: &mut Chars<'_>) -> String {
    let mut s = String::new();
    s.push('"');
    while let Some(n) = chs.next() {
        s.push(n);
        if n == '"' {
            break;
        }
        // Handle escape sequences.
        if n == '\\'
            && let Some(e) = chs.next()
        {
            s.push(e);
        }
    }
    s
}

/// Read a single-quoted or raw string body (opening quote already consumed)
/// up to the next `quote`. Returns the literal including both quotes.
fn take_plain_quoted(chs: &mut Chars<'_>, quote: char) -> String {
    let mut s = String::new();
    s.push(quote);
    for n in chs.by_ref() {
        s.push(n);
        if n == quote {
            break;
        }
    }
    s
}

/// Tokenise the **content** between a `{{` and the matching `}}`.
pub(super) fn lex_action(raw: &str) -> Vec<Tok> {
    let mut tokens = Vec::with_capacity(raw.len() / 4);
    let mut chs = raw.chars().peekable();

    while let Some(ch) = chs.next() {
        // Nested `{{` / `}}` inside an action.
        if ch == '{' && chs.peek() == Some(&'{') {
            chs.next();
            let trim = take_trim(&mut chs, Trim::Left);
            tokens.push(Tok::OpenAction(trim));
            continue;
        }
        if ch == '}' && chs.peek() == Some(&'}') {
            chs.next();
            let trim = take_trim(&mut chs, Trim::Right);
            tokens.push(Tok::CloseAction(trim));
            continue;
        }
        // Quoted string literals.
        match ch {
            '"' => tokens.push(Tok::QStr(QType::Dbl, take_dbl_quoted(&mut chs))),
            '\'' => tokens.push(Tok::QStr(QType::Sgl, take_plain_quoted(&mut chs, '\''))),
            '`' => tokens.push(Tok::QStr(QType::Raw, take_plain_quoted(&mut chs, '`'))),
            ' ' | '\t' | '\r' | '\n' => {
                // Collapse whitespace.
                while let Some(&n) = chs.peek()
                    && (n == ' ' || n == '\t' || n == '\r' || n == '\n')
                {
                    chs.next();
                }
                tokens.push(Tok::WS);
            }
            _ => tokens.push(Tok::Punct(ch)),
        }
    }

    tokens
}
