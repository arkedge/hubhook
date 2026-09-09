//! GitHub flavored markdown を Slack の mrkdwn に変換する。
//!
//! attachment の中では `blocks` が一切通らない (`markdown` は `internal_error`、
//! `rich_text` と `section` は `invalid_attachments`) ので、色バーを保ったまま
//! 整形するには自前で mrkdwn にするしかない。Slack の GitHub App も同じ方式で、
//! 縦の色線と太字を両立させている。
//!
//! 正規表現ではなくパーサを通すのは、コードブロックの中の `**` や `#` を
//! 装飾として誤認しないため。issue 本文には任意のテキストが来る。
//!
//! mrkdwn は方言なので、対応しない記法は落とさず近いものに寄せる。
//!
//! | markdown | mrkdwn |
//! | --- | --- |
//! | `# 見出し` | `*見出し*` (見出しは無い) |
//! | `**太字**` | `*太字*` |
//! | `*斜体*` | `_斜体_` |
//! | `~~打ち消し~~` | `~打ち消し~` |
//! | `[text](url)` | `<url\|text>` |
//! | `- 項目` | `• 項目` |
//! | `1. 項目` | `1. 項目` (番号付きリストは無い) |
//! | 表 | 行として並べる (見出し行は太字) |
//!
//! mrkdwn には文字を打ち消す記法が無いので、次は直せない。
//!
//! - 本文に書かれた `*` `_` `~` は Slack が装飾として解釈する。markdown で
//!   escape した記号 (`\*literal\*`) も escape が外れた状態で渡ってくるし、
//!   `~text~` は GitHub では打ち消しにならないが Slack ではなる。
//!
//!   フェンスと同じように幅ゼロの文字で無効化はしない。`foo_bar_baz` のような
//!   識別子に不可視文字が入ってコピペが壊れる。飲み込まれるフェンスと違って
//!   被害は装飾の食い違いに留まるので、治療の方が高くつく
//! - `<code>` の中身は markdown として解釈される。GitHub も同じなので描画は
//!   一致するが、monospace のスタイルは失う

use pulldown_cmark::{BlockQuoteKind, Event, Options, Parser, Tag, TagEnd};

/// ユーザが書いた文字列を Slack に渡せる形にする。
///
/// `&` `<` `>` は Slack のテキストで意味を持つので escape する。
/// <https://docs.slack.dev/messaging/formatting-message-text>
///
/// あわせてコードフェンスを無効化する。本文に裸の ``` があると、Slack が
/// フェンスとして読んで以降の本文と Assignees まで飲み込む。コードの中だけ
/// 気にしていると、散文に書かれた ``` を取りこぼす。ユーザ由来の文字列は
/// 全部この関数を通るので、ここで一度に守る。
///
/// こちらが組み立てる `*太字*` や `<url|label>` はこの関数を通らないので、
/// 無効化の対象にならない。
fn escape(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    neutralize_fences(&escaped)
}

/// リンクの URL 側。
///
/// `<url|label>` の `<` `>` `|` は Slack が区切りとして読むので、URL に
/// 入っているとリンク先が途中で切れる (`https://example.com/a|b` が
/// `https://example.com/a` になる)。percent-encode して渡す。
fn escape_url(url: &str) -> String {
    url.replace('&', "&amp;")
        .replace('|', "%7C")
        .replace('<', "%3C")
        .replace('>', "%3E")
}

/// 落とすと語がくっついてしまうタグに対して、代わりに置く文字。
///
/// `<br>` と、段落・箇条書き・表の行の閉じタグは改行にする。表のセルは
/// 改行だと縦に伸びるので空白にする。開きタグ側は前の要素の閉じタグで
/// 区切りが入るので見ない。
fn tag_separator(tag: &str) -> Option<char> {
    let name = tag
        .trim_start_matches('/')
        .split([' ', '\t', '\n', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    // 閉じタグを持たないので、開きタグで区切る
    if name == "br" || name == "hr" {
        return Some('\n');
    }
    if !tag.starts_with('/') {
        return None;
    }

    match name.as_str() {
        "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" => {
            Some('\n')
        }
        // 落とすと前後の塊がくっつく。`<pre>code</pre><p>after</p>` が
        // `codeafter` になる
        "pre" | "dl" | "dt" | "dd" | "ul" | "ol" | "table" | "thead" | "tbody" => Some('\n'),
        // <details><summary>Title</summary>Body</details> が TitleBody に
        // ならないようにする
        "summary" | "details" => Some('\n'),
        "td" | "th" => Some(' '),
        _ => None,
    }
}

/// `<` がタグを開くか。
///
/// HTML では `<` の次が英字・`/`・`!`・`?` のどれかでないとタグ名を始められず、
/// `<` は文字として表示される。`a < b > c` の `< b >` はタグではないので、
/// タグとして落とすと本文が消える。
fn opens_tag(next: Option<&u8>) -> bool {
    next.is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
}

/// 複数行に分かれたタグの、まだ閉じていない部分。
///
/// 読んだ位置と引用符の状態も持つ。継ぎ足すたびに頭から読み直すと、`<div` の
/// 後に属性の行が延々と続く本文で長さの 2 乗の時間がかかる。本文は誰でも
/// 書けるので、続きから読む。
struct PendingTag {
    text: String,
    /// ここまでは読んだ
    scanned: usize,
    /// 読んでいる途中の引用符
    quote: Option<u8>,
}

/// タグを閉じる `>` の位置。`from` から続きを読む。
///
/// 引用符の中の `>` は属性値の一部なのでタグの終わりではない。閉じていなければ
/// `None` で、その場合は次のイベントに続いている。
fn scan_tag(s: &str, from: usize, quote: &mut Option<u8>) -> Option<usize> {
    // 先頭の `<` は読まない。`"` `'` `>` は ASCII なので UTF-8 の後続バイトと
    // 衝突しない
    for (i, &c) in s.as_bytes().iter().enumerate().skip(from.max(1)) {
        match *quote {
            Some(q) if c == q => *quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => *quote = Some(c),
            None if c == b'>' => return Some(i),
            None => {}
        }
    }

    None
}

/// 数値参照の番号を文字にする。
///
/// HTML はそのまま Unicode の番号として読まない。0 と、文字にならない番号は
/// U+FFFD。素直に読むと `&#0;` が NUL になって、Slack に送る本文に制御文字が
/// 入る。
///
/// 0x80..=0x9F は Windows-1252 の記号に読み替える (`&#128;` は `€`)。
/// <https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-end-state>
fn from_code(code: u32) -> char {
    const C1: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];

    match code {
        0 => '\u{fffd}',
        0x80..=0x9f => C1[(code - 0x80) as usize],
        // 単独のサロゲートや範囲外は char にならない
        _ => char::from_u32(code).unwrap_or('\u{fffd}'),
    }
}

/// HTML の文字参照を戻す。
///
/// タグを外しただけの文字列には `&amp;` などが残っている。そのまま Slack 用に
/// escape すると `&amp;amp;` になって、画面に `&amp;` と出てしまう。
///
/// 1 回の走査で戻す。replace を並べると、戻した結果が次の replace に食われる
/// (`&#38;lt;` が `&lt;` を経て `<` になる)。HTML としては `&lt;` という文字列
/// なので、1 段だけ戻すのが正しい。
///
/// 名前付きの参照は全部持つには表か依存が要る。GitHub の本文で実際に見かける
/// ものだけ並べ、残りはそのまま出す (生で見えるが消えはしない)。
fn decode_refs(text: &str) -> String {
    const NAMED: &[(&str, char)] = &[
        ("amp", '&'),
        ("lt", '<'),
        ("gt", '>'),
        ("quot", '"'),
        ("apos", '\''),
        ("nbsp", ' '),
        ("copy", '\u{a9}'),
        ("reg", '\u{ae}'),
        ("deg", '\u{b0}'),
        ("middot", '\u{b7}'),
        ("times", '\u{d7}'),
        ("ndash", '\u{2013}'),
        ("mdash", '\u{2014}'),
        ("hellip", '\u{2026}'),
        ("laquo", '\u{ab}'),
        ("raquo", '\u{bb}'),
    ];

    let mut out = String::new();
    let mut rest = text;

    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];

        // 数値参照。`&#8230;` と `&#x2026;` の両方
        let numeric: Option<(char, &str)> = after.strip_prefix('#').and_then(|body| {
            let (digits, radix) = match body.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (body, 10),
            };
            // 数字の並びが参照。`;` は付いていれば取るが、無くても参照になる
            let end = digits
                .find(|c: char| !c.is_digit(radix))
                .unwrap_or(digits.len());
            if end == 0 {
                return None;
            }

            let c = match u32::from_str_radix(&digits[..end], radix) {
                Ok(code) => from_code(code),
                // u32 に収まらない番号は必ず範囲外
                Err(_) => '\u{fffd}',
            };
            let rest = &digits[end..];

            Some((c, rest.strip_prefix(';').unwrap_or(rest)))
        });

        if let Some((c, tail)) = numeric {
            out.push(c);
            rest = tail;
            continue;
        }

        let named = NAMED.iter().find_map(|(name, c)| {
            after
                .strip_prefix(*name)
                .and_then(|r| r.strip_prefix(';'))
                .map(|r| (*c, r))
        });

        match named {
            Some((c, tail)) => {
                out.push(c);
                rest = tail;
            }
            // 参照になっていないので、`&` はそのまま出す
            None => {
                out.push('&');
                rest = after;
            }
        }
    }

    out.push_str(rest);
    out
}

/// Slack のコードフェンスとして読まれるバックティックの連続を無効化する。
///
/// Slack のフェンスは ``` 固定で長さを変えられない。本文の中に ``` があると
/// フェンスを開いてしまい、以降の本文や Assignees まで飲み込む。
///
/// 幅ゼロの文字を挟んで、見た目を保ったまま連続を切る。
///
/// 2 本ごとに切る。``` を置き換えるだけだと、5 本や 8 本の連続で置換後の
/// 末尾と残りが繋がって ``` に戻ってしまう。
fn neutralize_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut run = 0;

    for c in text.chars() {
        if c == '`' {
            if run == 2 {
                out.push('\u{200b}');
                run = 0;
            }
            run += 1;
        } else {
            run = 0;
        }
        out.push(c);
    }

    out
}

/// mrkdwn のリンク。
///
/// ラベルが無いときは URL だけ出す。`<url|>` は空ラベルになって何も見えない。
///
/// 絶対 URL でない行き先はリンクにしない。`[Usage](#usage)` を
/// `<#usage|Usage>` にすると、mrkdwn では **Slack のチャンネル参照**として
/// 解釈されて、無いチャンネルへのリンクになる。相対パスも辿れない。
fn link(url: &str, label: &str) -> String {
    if !is_absolute(url) {
        return label.to_string();
    }
    if label.trim().is_empty() {
        format!("<{}>", escape_url(url))
    } else {
        format!("<{}|{}>", escape_url(url), label)
    }
}

/// Slack がリンクとして辿れる行き先か。
fn is_absolute(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|p| lower.starts_with(p))
}

/// タグから属性値を取り出す。
///
/// 生 HTML の `<a href>` や `<img src>` の行き先を捨てないために使う。
///
/// 属性を順に読む。名前を検索すると、別の属性値の中の文字列
/// (`<a title="note href=https://evil.example.com">`) を属性と見て、行き先を
/// 捏造してしまう。引用符の中は値なので、名前を探す対象にしない。
fn attr(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut i = 0;

    // 先頭はタグ名で属性ではない
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }

    while i < bytes.len() {
        // 属性の前の空白と、閉じ方を表す `/`
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }

        let from = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }

        // 名前が読めなければ、これ以上属性は無い
        if i == from {
            return None;
        }
        let found = tag[from..i].eq_ignore_ascii_case(name);

        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }

        // 値を持たない属性 (`<td nowrap>`)
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }

        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }

        let value = match bytes[i] {
            q @ (b'"' | b'\'') => {
                let from = i + 1;
                // 閉じていない引用符は壊れたタグなので、行き先は使わない
                let end = from + tag[from..].find(q as char)?;
                i = end + 1;
                &tag[from..end]
            }
            // 引用符の無い値は空白か `>` まで。末尾の `/` は値の一部で、
            // 閉じ方を表す `/` は空白で区切られている
            _ => {
                let from = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                &tag[from..i]
            }
        };

        if found {
            return Some(value.to_string());
        }
    }

    None
}

/// 変換したものを組み立てる。
///
/// リンクのラベルや表のセルは「中身を全部読んでから」出力を決めるので、
/// 書き込み先をスタックにして切り替える。
struct Renderer {
    /// 末尾が現在の書き込み先
    bufs: Vec<String>,
    /// 入れ子のリスト。`Some` は番号付きで、次に振る番号を持つ
    lists: Vec<Option<u64>>,
    /// 読んでいる途中のリンク先
    links: Vec<String>,
    /// 組み立て中の表
    table: Option<Table>,
    /// 読んでいる途中の `<a href>` の行き先。
    pending_href: Option<String>,
    /// HTML コメントの途中か。
    ///
    /// pulldown-cmark は HTML ブロックを行ごとに別のイベントで渡すので、
    /// `<!--` と `-->` が別のイベントに分かれる。イベント単位で見ると
    /// 途中の行が本文として出てしまう。
    in_html_comment: bool,
    /// リスト項目の印を書いた直後か。
    ///
    /// 項目の中に段落が来ると (blank line を含むリスト) 段落として空行を
    /// 入れてしまい、`• ` と本文が離れてしまう。
    after_marker: bool,
    /// 複数行に分かれたタグの、まだ閉じていない部分
    pending_tag: Option<PendingTag>,
    /// 項目の 2 行目以降に付ける字下げ。入れ子の分だけ積む
    item_pads: Vec<String>,
    /// 引用の中か。字下げは引用記法の外に付けるので、中では入れない
    quote_depth: usize,
    /// コードブロックの中か。中身に空白を足すとコードが変わるので入れない
    code_depth: usize,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
}

impl Renderer {
    fn new() -> Self {
        Self {
            bufs: vec![String::new()],
            lists: Vec::new(),
            links: Vec::new(),
            table: None,
            pending_href: None,
            in_html_comment: false,
            after_marker: false,
            pending_tag: None,
            item_pads: Vec::new(),
            quote_depth: 0,
            code_depth: 0,
        }
    }

    /// 書き込み先に足す。
    ///
    /// 継ぎ目で ``` ができないようにする。無効化は 1 つの文字列の中しか見ない
    /// ので、前が `` ` `` で終わって次が `` ` `` で始まると、どちらも 3 本未満
    /// でも繋がって Slack のフェンスになる。イベントが分かれるだけで起きる
    /// (`` \` `` と HTML コメントと `` \`\` `` など)。
    ///
    /// 幅ゼロの文字を挟む。こちらが組み立てるコード span やコードブロックの
    /// `` ` `` も対象にするが、挟まるのは前の文字列との間なので、区切りとしては
    /// そのまま残る。
    fn push(&mut self, s: &str) {
        let buf = self.bufs.last_mut().expect("書き込み先が無い");

        let tail = buf.chars().rev().take_while(|c| *c == '`').count();
        let head = s.chars().take_while(|c| *c == '`').count();
        if tail > 0 && head > 0 && tail + head >= 3 {
            buf.push('\u{200b}');
        }

        buf.push_str(s);
    }

    fn open(&mut self) {
        self.bufs.push(String::new());
    }

    fn close(&mut self) -> String {
        self.bufs.pop().expect("閉じる書き込み先が無い")
    }

    /// 直前が空行になるまで改行を足す。段落の間を空けるのに使う。
    fn blank_line(&mut self) {
        let cur = self.bufs.last().expect("書き込み先が無い");
        if cur.is_empty() {
            return;
        }
        if cur.ends_with("\n\n") {
            return;
        }
        if cur.ends_with('\n') {
            self.push("\n");
        } else {
            self.push("\n\n");
        }
    }

    /// 行の頭なら、項目の字下げを入れる。
    ///
    /// 入れないと、折り返した行や 2 つめの段落が項目の外に見える
    /// (`- first\n  continuation` が `• first\ncontinuation` になる)。
    ///
    /// 行の途中では何もしない。印を書いた直後は行の途中なので、印と中身の
    /// 間に字下げが入ることはない。
    ///
    /// コードブロックからは呼ばない。フェンスや中身に空白を足すと、コード
    /// そのものが変わってしまう。
    /// 行の頭でしか動かないので、書き出す側が何度呼んでも二重にならない。
    fn line_pad(&mut self) {
        // 引用の中身には入れない。引用記法の前に付けるので、中に入れると
        // `> ` の後ろが空くだけになる
        if self.quote_depth > 0 {
            return;
        }

        // コードブロックの中身も字下げしない。空白を足すとコードが変わる
        if self.code_depth > 0 {
            return;
        }

        let cur = self.bufs.last().expect("書き込み先が無い");
        if !cur.ends_with('\n') {
            return;
        }

        if let Some(pad) = self.item_pads.last().cloned() {
            self.push(&pad);
        }
    }

    /// 複数行の文字列を足す。行ごとに項目の字下げを入れる。
    ///
    /// 表や生 HTML の区切りのように、1 回の push に改行が混ざるものがある。
    /// そのままだと 2 行目以降が項目の外に見える。
    fn push_lines(&mut self, s: &str) {
        self.line_pad();

        let pad = match self.item_pads.last() {
            Some(pad) if self.quote_depth == 0 => pad.clone(),
            _ => {
                self.push(s);
                return;
            }
        };

        // 空行は字下げしない。末尾に空白だけが残る
        let padded = s
            .split('\n')
            .enumerate()
            .map(|(i, l)| {
                if i == 0 || l.is_empty() {
                    l.to_string()
                } else {
                    format!("{pad}{l}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        self.push(&padded);
    }

    fn newline(&mut self) {
        let cur = self.bufs.last().expect("書き込み先が無い");
        if !cur.is_empty() && !cur.ends_with('\n') {
            self.push("\n");
        }
    }

    /// HTML からタグとコメントを落として、見える文字だけ返す。
    ///
    /// issue のテンプレートは `<!-- 説明 -->` や `<details>` を含む。タグを
    /// そのまま出すと読めないが、イベントごと捨てると中の文字まで消える。
    ///
    /// タグの終わりは最初の `>` ではない。`<span title="a > b">` のように
    /// 属性値の中に `>` が入るので、引用符の中は読み飛ばす。
    /// タグを落として、落とすと消えてしまうものだけ残す。
    fn drop_tag(&mut self, tag: &str, out: &mut String) {
        // 区切りを意味するタグは、落とすと語がくっつく (`a<br>b` -> `ab`)
        if let Some(sep) = tag_separator(tag) {
            out.push(sep);
        }

        // タグを落とすと行き先まで消える。`<img>` は中に文字が無いので
        // alt か src を出さないと本文が空になる。
        out.push_str(&self.html_target(tag));
    }

    fn html_text(&mut self, html: &str) -> String {
        let mut out = String::new();
        let bytes = html.as_bytes();
        let mut i = 0;

        while i < bytes.len() {
            // コメントの中身は落とす。前のイベントから続いていることもある
            if self.in_html_comment {
                match html[i..].find("-->") {
                    Some(j) => {
                        i += j + 3;
                        self.in_html_comment = false;
                    }
                    // まだコメントの中。次のイベントへ持ち越す
                    None => return decode_refs(&out),
                }
                continue;
            }

            // タグが前のイベントで閉じていなかった場合、続きを足して閉じるまで待つ
            if let Some(mut pending) = self.pending_tag.take() {
                let base = pending.text.len();
                pending.text.push_str(&html[i..]);

                let Some(end) = scan_tag(&pending.text, pending.scanned, &mut pending.quote) else {
                    pending.scanned = pending.text.len();
                    self.pending_tag = Some(pending);
                    return decode_refs(&out);
                };

                self.drop_tag(&pending.text[1..end], &mut out);

                // 前のイベントまでに閉じる `>` は無かったので end はその外にある
                i += end + 1 - base;
                continue;
            }

            if html[i..].starts_with("<!--") {
                self.in_html_comment = true;
                i += 4;
                continue;
            }

            if bytes[i] == b'<' && opens_tag(bytes.get(i + 1)) {
                let rest = &html[i..];
                let mut quote = None;

                let Some(end) = scan_tag(rest, 1, &mut quote) else {
                    // 閉じていないので次のイベントに続く。属性が複数行に
                    // 分かれているだけなので、捨てずに持ち越す
                    self.pending_tag = Some(PendingTag {
                        text: rest.to_string(),
                        scanned: rest.len(),
                        quote,
                    });
                    return decode_refs(&out);
                };

                self.drop_tag(&html[i + 1..i + end], &mut out);
                i += end + 1;
                continue;
            }

            let ch = html[i..].chars().next().expect("char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }

        decode_refs(&out)
    }

    /// 生 HTML のタグから、落とすと消えてしまう情報を取り出す。
    ///
    /// `<a href>` はラベルがタグの外にあるので、閉じタグまで待って URL を
    /// 添える。`<img>` は中に文字が無いので、alt か src をここで出す。
    fn html_target(&mut self, tag: &str) -> String {
        let name = tag
            .trim_start_matches('/')
            .split([' ', '\t', '\n', '/'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        match name.as_str() {
            "a" if !tag.starts_with('/') => {
                // scheme の判定は参照を戻した方で行う。`&#104;ttps://…` は
                // HTML では https なので、生のままだと相対 URL とみなして
                // リンク先を落としてしまう。出す値は生のままにして、戻すのは
                // html_text の最後の 1 回に任せる
                self.pending_href = attr(tag, "href").filter(|u| is_absolute(&decode_refs(u)));
                String::new()
            }
            // 属性値は HTML のまま返す。文字参照を戻すのは html_text の最後の
            // 1 回だけで、ここで戻すと二重になる。escape も呼び出し側が行う。
            "a" => match self.pending_href.take() {
                Some(url) => format!(" ({url})"),
                None => String::new(),
            },
            "img" => {
                let alt = attr(tag, "alt").unwrap_or_default();
                if !alt.trim().is_empty() {
                    return alt;
                }
                attr(tag, "src").unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    /// ブロックの始まり。段落の区切りを入れる。
    ///
    /// ただしリスト項目の印や脚注のラベルを書いた直後は入れない。入れると印と
    /// 中身が離れて `• ` や `[^1]: ` だけの行ができる。段落に限らず、見出しや
    /// コードブロックが先頭に来ることもある。
    fn block_start(&mut self) {
        if self.after_marker {
            self.after_marker = false;
        } else {
            self.blank_line();
        }
    }

    /// リストの入れ子に応じた字下げ。
    fn indent(&self) -> String {
        "    ".repeat(self.lists.len().saturating_sub(1))
    }
}

/// GitHub flavored markdown を mrkdwn にする。
pub fn from_markdown(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    // `> [!NOTE]` のような GitHub alerts。名前に反してこれだけを指す
    opts.insert(Options::ENABLE_GFM);
    // 有効にしないと `[^1]: 定義` がリンク参照定義として扱われ、定義ごと消える
    opts.insert(Options::ENABLE_FOOTNOTES);

    let mut r = Renderer::new();

    for event in Parser::new_ext(md, opts) {
        match event {
            Event::Start(tag) => start(&mut r, tag),
            Event::End(tag) => end(&mut r, tag),
            Event::Text(t) => {
                let escaped = escape(&t);
                // 改行を出したのが別のイベントでも行頭を揃える (`<br>` の後など)
                r.line_pad();
                r.push(&escaped);
            }
            Event::Code(t) => {
                let escaped = escape(&t);
                r.line_pad();

                // mrkdwn のコード span は ` で囲む以外に書き方が無いので、
                // 中に ` が残っていると区切りが壊れる。素のテキストで出す。
                if escaped.contains('`') {
                    r.push(&escaped);
                } else {
                    r.push(&format!("`{escaped}`"));
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                r.push("\n");
                r.line_pad();
            }
            // Slack に水平線は無い。段落の切れ目としてだけ扱う
            Event::Rule => r.block_start(),
            Event::TaskListMarker(done) => {
                r.push(if done { "☑ " } else { "☐ " });

                // 折り返した行を課題の文字に揃える。印の分だけ足す
                if let Some(pad) = r.item_pads.last_mut() {
                    pad.push_str("  ");
                }
            }
            // Slack に脚注は無いので、markdown の書き方をそのまま残す
            Event::FootnoteReference(label) => {
                let escaped = escape(&label);
                r.push(&format!("[^{escaped}]"));
            }
            // タグは落とすが中の文字は残す。pulldown-cmark は HTML ブロックを
            // まとめて 1 つのイベントで渡すので、丸ごと捨てると本文が消える
            Event::Html(h) | Event::InlineHtml(h) => {
                let visible = r.html_text(&h);
                if !visible.is_empty() {
                    let escaped = escape(&visible);
                    // タグの区切りで改行が混ざる (`<br>` など)
                    r.push_lines(&escaped);
                }
            }
            _ => {}
        }
    }

    let out = r.close();
    out.trim().to_string()
}

fn start(r: &mut Renderer, tag: Tag) {
    match tag {
        Tag::Paragraph => {
            r.block_start();
            // 項目の 2 つめの段落も項目の中に見せる
            r.line_pad();
        }
        // 見出しが無いので太字で代用する。中身を読んでから決めるので開く
        Tag::Heading { .. } => {
            r.block_start();
            r.open();
        }
        Tag::Strong => r.push("*"),
        Tag::Emphasis => r.push("_"),
        Tag::Strikethrough => r.push("~"),
        // 中身を読んでから囲むかを決めるので開く
        Tag::CodeBlock(_) => {
            r.block_start();
            r.open();
            r.code_depth += 1;
        }
        Tag::BlockQuote(kind) => {
            r.block_start();
            // 中身を組み立ててから各行に "> " を付ける
            r.open();
            r.quote_depth += 1;

            // GitHub alerts (`> [!NOTE]` など)。種類はパーサが食べてしまうので、
            // 落とさずに見出しとして書き戻す
            if let Some(kind) = kind {
                let label = match kind {
                    BlockQuoteKind::Note => "NOTE",
                    BlockQuoteKind::Tip => "TIP",
                    BlockQuoteKind::Important => "IMPORTANT",
                    BlockQuoteKind::Warning => "WARNING",
                    BlockQuoteKind::Caution => "CAUTION",
                };
                r.push(&format!("*{label}*\n"));
            }
        }
        Tag::List(first) => {
            // 入れ子のリストは項目の続きなので、空行で切らない
            if r.lists.is_empty() {
                r.blank_line();
            } else {
                r.after_marker = false;
                r.newline();
            }
            r.lists.push(first);
        }
        Tag::Item => {
            r.newline();
            let indent = r.indent();
            let marker = match r.lists.last_mut() {
                Some(Some(n)) => {
                    let marker = format!("{n}. ");
                    *n += 1;
                    marker
                }
                _ => "• ".to_string(),
            };
            r.push(&format!("{indent}{marker}"));

            // 2 行目以降を中身の頭に揃える
            r.item_pads
                .push(format!("{indent}{}", " ".repeat(marker.chars().count())));
            r.after_marker = true;
        }
        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
            r.links.push(dest_url.to_string());
            r.open();
        }
        Tag::FootnoteDefinition(label) => {
            r.block_start();
            let escaped = escape(&label);
            r.push(&format!("[^{escaped}]: "));

            // 定義の中身は段落なので、印の直後だと伝えないと `[^1]: ` だけの
            // 行になって定義が離れる
            r.after_marker = true;
        }
        Tag::Table(_) => {
            r.block_start();
            r.table = Some(Table::default());
        }
        Tag::TableCell => r.open(),
        _ => {}
    }
}

fn end(r: &mut Renderer, tag: TagEnd) {
    match tag {
        // HTML ブロックは空行で終わるので、そこで閉じていないタグやコメントは
        // 閉じないまま終わる。持ち越したままだと、後から来た別のタグの頭に
        // くっついて、そのタグの行き先まで落としてしまう
        TagEnd::HtmlBlock => {
            r.pending_tag = None;
            r.in_html_comment = false;
        }
        TagEnd::Paragraph => r.blank_line(),
        TagEnd::Heading(_) => {
            let inner = r.close();
            let inner = inner.trim();

            // 中身に太字が混ざっていたら囲まない。`# **重要** の話` を囲むと
            // `**重要* の話*` になって区切りが重なり、mrkdwn として壊れる。
            // 見出し全体の太字は失うが、記法が壊れるより良い。
            if inner.is_empty() {
                // 空の見出しで `**` を作らない
            } else if inner.contains('*') {
                r.push(inner);
            } else {
                r.push(&format!("*{inner}*"));
            }
            r.blank_line();
        }
        TagEnd::Strong => r.push("*"),
        TagEnd::Emphasis => r.push("_"),
        TagEnd::Strikethrough => r.push("~"),
        TagEnd::CodeBlock => {
            r.code_depth -= 1;
            let code = r.close();
            // 閉じフェンスの直前の改行 1 つだけ落とす。trim_end だと
            // コードの一部である末尾の空白や空行まで消える
            let code = code.strip_suffix('\n').unwrap_or(&code);

            // Slack のコードブロックは ``` 固定で長さを変えられない。中に ```
            // があると途中で閉じてしまう。囲まずに出すと、その ``` が今度は
            // フェンスを開いて以降の本文まで飲み込むので、もっと悪い。
            //
            // 見た目を保ったまま無効化する。バックティックの間に幅ゼロの文字を
            // 挟むと、Slack はフェンスとして読まない。
            let code = neutralize_fences(code);
            r.push(&format!("```\n{code}\n```"));
            r.blank_line();
        }
        TagEnd::BlockQuote(_) => {
            r.quote_depth -= 1;
            let inner = r.close();

            // 項目の中の引用は、字下げを引用記法の前に付ける。付けないと
            // 2 行目以降の ">" が行頭に来て、項目の外の引用に見える
            let pad = r.item_pads.last().cloned().unwrap_or_default();

            // 引用記法の ">" は生で置く。`&gt;` にすると Slack は引用として
            // 解釈せず、リテラルの ">" を表示する
            let quoted = inner
                .trim_end()
                .lines()
                .enumerate()
                .map(|(i, l)| {
                    // 1 行目は印の直後なので字下げしない
                    let pad = if i == 0 { "" } else { pad.as_str() };
                    if l.is_empty() {
                        format!("{pad}>")
                    } else {
                        format!("{pad}> {l}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            r.push(&quoted);
            r.blank_line();
        }
        TagEnd::List(_) => {
            r.lists.pop();
            // 外側のリストが続いているなら、まだ段落は終わっていない
            if r.lists.is_empty() {
                r.blank_line();
            } else {
                r.newline();
            }
        }
        TagEnd::Item => {
            r.item_pads.pop();
            r.newline();
        }
        TagEnd::Link => {
            let label = r.close();
            let url = r.links.pop().unwrap_or_default();
            r.push(&link(&url, &label));
        }
        TagEnd::Image => {
            let alt = r.close();
            let url = r.links.pop().unwrap_or_default();

            // リンクの中の画像 (`[![CI](badge)](build)` のようなバッジ) を
            // そのままリンクにすると `<build|<badge|CI>>` になる。mrkdwn の
            // リンク記法は入れ子を扱えず、内側の `>` で外側が閉じてしまう。
            // 外側のリンクが開いているなら alt だけ置く。
            if r.links.is_empty() {
                r.push(&link(&url, &alt));
            } else {
                r.push(&alt);
            }
        }
        TagEnd::TableCell => {
            let cell = r.close();
            if let Some(t) = r.table.as_mut() {
                t.row.push(cell.trim().to_string());
            }
        }
        TagEnd::TableHead | TagEnd::TableRow => {
            if let Some(t) = r.table.as_mut() {
                let row = std::mem::take(&mut t.row);
                t.rows.push(row);
            }
        }
        TagEnd::FootnoteDefinition => r.blank_line(),
        TagEnd::Table => {
            if let Some(t) = r.table.take() {
                let rendered = render_table(&t.rows);
                r.push_lines(&rendered);
                r.blank_line();
            }
        }
        _ => {}
    }
}

/// 表は行をそのまま並べる。
///
/// mrkdwn に表は無い。等幅ブロックに入れて桁を揃える手もあるが、
/// ブロックの中では mrkdwn が解釈されないので、セルの中のリンクや強調が
/// 記法のまま見えてしまう。桁揃え自体も、日本語や絵文字では文字数と表示幅が
/// 一致しないので守れない。揃えるのを諦めて、セルの中身を活かす。
///
/// 見出し行は太字にして、本体と見分けられるようにする。
fn render_table(rows: &[Vec<String>]) -> String {
    let mut lines = Vec::new();

    for (i, row) in rows.iter().enumerate() {
        let cells: Vec<String> = row
            .iter()
            .map(|c| {
                // 中身に太字が混ざっていたら囲まない (見出しと同じ理由)
                if i == 0 && !c.is_empty() && !c.contains('*') {
                    format!("*{c}*")
                } else {
                    c.clone()
                }
            })
            .collect();
        lines.push(cells.join(" | "));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::from_markdown;

    /// mrkdwn に見出しは無いので太字にする。
    #[test]
    fn headings_become_bold() {
        assert_eq!(from_markdown("## 概要"), "*概要*");
        assert_eq!(from_markdown("# h1"), "*h1*");
        assert_eq!(from_markdown("###### h6"), "*h6*");
    }

    /// 強調の方言が逆なので入れ替える。
    ///
    /// mrkdwn では `*x*` が太字、`_x_` が斜体。markdown のまま渡すと
    /// `**太字**` が「`*` 付きの斜体」として出る。
    #[test]
    fn emphasis_dialects_are_swapped() {
        assert_eq!(from_markdown("**太字**"), "*太字*");
        assert_eq!(from_markdown("*斜体*"), "_斜体_");
        assert_eq!(from_markdown("~~打ち消し~~"), "~打ち消し~");
    }

    #[test]
    fn links_use_the_mrkdwn_form() {
        assert_eq!(
            from_markdown("[text](https://example.com)"),
            "<https://example.com|text>"
        );
    }

    /// ラベルが無いリンクは URL だけにすること。
    ///
    /// `<url|>` は空ラベルになって何も見えない。
    #[test]
    fn links_without_a_label_show_the_url() {
        assert_eq!(
            from_markdown("[](https://example.com)"),
            "<https://example.com>"
        );
    }

    /// **コードブロックの中は書き換えないこと。**
    ///
    /// 正規表現で一括置換すると、コードとして書かれた `**` や `[]()` が
    /// 装飾に変えられてしまう。パーサを通しているのはこのため。
    #[test]
    fn code_blocks_are_left_alone() {
        let out = from_markdown("```\n**not bold** and [not a link](x)\n```");

        assert!(out.contains("**not bold**"), "書き換えられている: {out}");
        assert!(out.contains("[not a link](x)"), "書き換えられている: {out}");
    }

    #[test]
    fn inline_code_is_left_alone() {
        assert_eq!(from_markdown("`**x**`"), "`**x**`");
    }

    #[test]
    fn bullets_use_a_dot() {
        assert_eq!(from_markdown("- a\n- b"), "• a\n• b");
    }

    /// mrkdwn に番号付きリストは無いので、番号を文字として残す。
    #[test]
    fn ordered_lists_keep_their_numbers() {
        assert_eq!(from_markdown("1. a\n2. b"), "1. a\n2. b");
    }

    /// どの長さのバックティックの連続も切ること。
    ///
    /// ``` を置き換えるだけだと、5 本や 8 本で置換後の末尾と残りが繋がって
    /// ``` に戻り、Slack のフェンスとして読まれる。
    #[test]
    fn every_backtick_run_is_split() {
        for n in 3..=10 {
            let out = super::neutralize_fences(&"`".repeat(n));
            assert!(!out.contains("```"), "n={n} で繋がっている: {out:?}");
        }
    }

    /// 生 HTML の改行の後も項目の字下げを続けること。
    ///
    /// 改行を出すのとその後の文字が別のイベントなので、書き出す側で行頭を
    /// 揃えないとこぼれる。
    #[test]
    fn a_break_inside_an_item_keeps_the_indent() {
        assert_eq!(from_markdown("- first<br>second"), "• first\n  second");
    }

    /// 項目の中の表も項目の字下げに揃えること。
    #[test]
    fn a_table_inside_an_item_keeps_the_indent() {
        assert_eq!(
            from_markdown("- item\n\n  | h |\n  | --- |\n  | v |"),
            "• item\n\n  *h*\n  v"
        );
    }

    /// 項目の中のコードブロックは字下げしないこと。
    ///
    /// フェンスや中身に空白を足すと、コードそのものが変わってしまう。
    #[test]
    fn code_inside_an_item_is_not_indented() {
        assert_eq!(
            from_markdown("- a\n\n  ```\n  code line\n  ```"),
            "• a\n\n```\ncode line\n```"
        );
    }

    /// 課題の折り返しをチェックボックスの後ろに揃えること。
    ///
    /// 印の幅だけを見ていると、チェックボックスの下に続きが来る。
    #[test]
    fn a_task_items_continuation_clears_the_checkbox() {
        assert_eq!(
            from_markdown("- [x] task text\n  wrapped line"),
            "• ☑ task text\n    wrapped line"
        );
    }

    /// 項目の中の引用は、字下げを引用記法の前に付けること。
    ///
    /// 中に入れると `> ` の後ろが空くだけで、2 行目以降の `>` が行頭に来て
    /// 項目の外の引用に見える。
    #[test]
    fn a_quote_in_an_item_keeps_the_item_indent() {
        assert_eq!(
            from_markdown("- > first\n  > second"),
            "• > first\n  > second"
        );
    }

    /// 折り返した行を項目の中に見せること。
    ///
    /// 字下げしないと `• first\ncontinuation` になって、続きが項目の外に
    /// 見える。番号付きなら番号の幅だけ下げる。
    #[test]
    fn continuation_lines_stay_inside_the_item() {
        assert_eq!(
            from_markdown("- first\n  continuation\n- next"),
            "• first\n  continuation\n• next"
        );
        assert_eq!(
            from_markdown("1. first\n   continuation\n2. next"),
            "1. first\n   continuation\n2. next"
        );
    }

    /// 項目の 2 つめの段落も項目の中に見せること。
    #[test]
    fn a_second_paragraph_stays_inside_the_item() {
        assert_eq!(
            from_markdown("- a\n\n  second\n\n- b"),
            "• a\n\n  second\n\n• b"
        );
    }

    /// 入れ子の項目の折り返しは、内側の中身の頭に揃えること。
    #[test]
    fn a_nested_items_continuation_follows_the_inner_marker() {
        assert_eq!(
            from_markdown("- outer\n  - inner\n    continuation"),
            "• outer\n    • inner\n      continuation"
        );
    }

    /// 入れ子は字下げで表す。項目の間に空行を挟まないこと。
    #[test]
    fn nested_lists_are_indented() {
        assert_eq!(from_markdown("- a\n    - b\n- c"), "• a\n    • b\n• c");
    }

    /// Slack で意味を持つ 3 文字を escape すること。
    ///
    /// <https://docs.slack.dev/messaging/formatting-message-text>
    #[test]
    fn special_characters_are_escaped() {
        assert_eq!(from_markdown("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    /// URL の `&` も escape するが、リンクの区切りは壊さないこと。
    #[test]
    fn link_urls_escape_ampersands() {
        assert_eq!(
            from_markdown("[q](https://example.com/?a=1&b=2)"),
            "<https://example.com/?a=1&amp;b=2|q>"
        );
    }

    /// 引用記法の `>` は生で置くこと。
    ///
    /// `&gt;` にすると Slack は引用として解釈せず、リテラルの `>` を表示する。
    #[test]
    fn quotes_use_the_raw_marker() {
        assert_eq!(from_markdown("> line1\n> line2"), "> line1\n> line2");
    }

    /// GitHub alerts の種類を落とさないこと。
    ///
    /// パーサが `[!NOTE]` を食べて種類として返すので、書き戻さないと
    /// 「注意書きである」という情報が消える。
    #[test]
    fn github_alerts_keep_their_label() {
        let out = from_markdown("> [!WARNING]\n> 危ない");

        assert!(out.contains("*WARNING*"), "ラベルが消えている: {out}");
        assert!(out.contains("> 危ない"), "本文が引用でない: {out}");
    }

    #[test]
    fn task_list_markers_become_boxes() {
        assert_eq!(
            from_markdown("- [x] done\n- [ ] todo"),
            "• ☑ done\n• ☐ todo"
        );
    }

    /// 表は行として出すこと。
    ///
    /// 等幅ブロックに入れて桁を揃えると、ブロックの中では mrkdwn が解釈
    /// されないので、セルのリンクや強調が記法のまま見えてしまう。
    #[test]
    fn tables_are_rendered_as_rows() {
        assert_eq!(
            from_markdown("| a | bb |\n| --- | --- |\n| 1 | 2 |"),
            "*a* | *bb*\n1 | 2"
        );
    }

    /// 表のセルの中の書式が生きていること。
    #[test]
    fn table_cells_keep_their_formatting() {
        let out = from_markdown(
            "| name | link |\n| --- | --- |\n| **bold** | [text](https://example.com) |",
        );

        assert!(out.contains("*bold*"), "強調が消えている: {out}");
        assert!(
            out.contains("<https://example.com|text>"),
            "リンクが消えている: {out}"
        );
    }

    /// URL に区切り文字が入っていてもリンク先が切れないこと。
    ///
    /// `<url|label>` の `|` `<` `>` は Slack が区切りとして読むので、URL に
    /// そのまま入れるとリンク先が途中で終わる。
    #[test]
    fn link_urls_escape_the_delimiters() {
        assert_eq!(
            from_markdown("[x](https://example.com/a|b)"),
            "<https://example.com/a%7Cb|x>"
        );
        assert_eq!(
            from_markdown("[x](https://example.com/a>b)"),
            "<https://example.com/a%3Eb|x>"
        );
    }

    /// リンクの中の画像を入れ子にしないこと。
    ///
    /// `[![CI](badge)](build)` のようなバッジは GitHub の本文でよく使われる。
    /// 入れ子にすると内側の `>` で外側のリンクが閉じて崩れる。
    #[test]
    fn images_inside_links_are_not_nested() {
        assert_eq!(
            from_markdown("[![CI](https://example.com/badge.svg)](https://example.com/build)"),
            "<https://example.com/build|CI>"
        );
    }

    /// 単独の画像はリンクにすること。
    #[test]
    fn standalone_images_become_links() {
        assert_eq!(
            from_markdown("![alt](https://example.com/x.png)"),
            "<https://example.com/x.png|alt>"
        );
    }

    /// コード span の中に ` があっても区切りを壊さないこと。
    ///
    /// mrkdwn には ` で囲む以外の書き方が無いので、囲むのを諦める。
    #[test]
    fn code_containing_a_backtick_is_not_fenced() {
        assert_eq!(from_markdown("`` a`b ``"), "a`b");
    }

    /// 中身が既に太字の見出しを二重にしないこと。
    ///
    /// `**重要**` になると mrkdwn では太字として解釈されない。
    #[test]
    fn already_bold_headings_are_not_wrapped_again() {
        assert_eq!(from_markdown("# **重要**"), "*重要*");
        assert_eq!(from_markdown("# 普通"), "*普通*");
    }

    /// HTML のタグは落とすが、中の文字は残すこと。
    ///
    /// pulldown-cmark は HTML ブロックをまとめて 1 つのイベントで渡すので、
    /// イベントごと捨てると本文まで消える。
    #[test]
    fn html_tags_are_stripped_but_text_is_kept() {
        assert_eq!(from_markdown("<b>important</b>"), "important");
        assert!(
            from_markdown("<table><tr><td>important</td></tr></table>").contains("important"),
            "HTML の表の中身が消えている"
        );
    }

    /// 属性値の中の `>` でタグを切らないこと。
    #[test]
    fn tag_attributes_may_contain_gt() {
        assert_eq!(from_markdown(r#"<span title="a > b">text</span>"#), "text");
    }

    /// HTML の文字参照を二重に escape しないこと。
    ///
    /// タグを外した文字列には `&amp;` が残っているので、そのまま escape すると
    /// 画面に `&amp;` と出る。
    #[test]
    fn html_character_references_are_decoded_once() {
        assert_eq!(from_markdown("<b>A &amp; B</b>"), "A &amp; B");
        assert_eq!(from_markdown("<b>&lt;tag&gt;</b>"), "&lt;tag&gt;");
    }

    /// 改行を意味するタグを落として語をくっつけないこと。
    #[test]
    fn line_breaking_tags_keep_the_break() {
        assert_eq!(from_markdown("first<br>second"), "first\nsecond");
        assert_eq!(from_markdown("<p>a</p><p>b</p>"), "a\nb");
    }

    /// 閉じていないタグをブロックの外まで持ち越さないこと。
    ///
    /// HTML ブロックは空行で終わる。持ち越すと、後の `<a href>` の頭に
    /// くっついてリンク先が落ちる。
    #[test]
    fn an_unclosed_tag_does_not_leak_into_the_next_block() {
        let out = from_markdown("<div\n\n<a href=\"https://example.com\">label</a>");

        assert!(out.contains("label"), "ラベルが消えている: {out:?}");
        assert!(
            out.contains("https://example.com"),
            "リンク先が消えている: {out:?}"
        );
    }

    /// イベントを跨いだ引用符の中の `>` でタグを閉じないこと。
    ///
    /// 続きから読むので、引用符の途中で行が変わっても状態を引き継ぐ必要が
    /// ある。閉じたと思うと属性値の残りが本文に出る。
    #[test]
    fn a_quote_spanning_events_keeps_its_state() {
        let out = from_markdown("<div title=\"a\n b > c\">\nvisible\n</div>");

        assert_eq!(out, "visible");
    }

    /// 複数行に分かれたタグの属性が本文に出ないこと。
    ///
    /// HTML ブロックは pulldown-cmark が行ごとにイベントを分けるので、閉じて
    /// いないタグを持ち越さないと 2 行目以降が文字として出る。
    #[test]
    fn a_tag_split_across_lines_is_dropped_whole() {
        let out = from_markdown("<div\n class=\"foo\">\nvisible\n</div>\nafter");

        assert!(!out.contains("class"), "属性が出ている: {out:?}");
        assert!(out.contains("visible"), "中身が消えている: {out:?}");
        assert!(out.ends_with("after"), "後ろが消えている: {out:?}");
    }

    /// 複数行に分かれた `<img>` からも alt を出すこと。
    #[test]
    fn a_multiline_img_keeps_its_alt() {
        assert_eq!(from_markdown("<img\n alt=\"shot\"\n src=\"x\">"), "shot");
    }

    /// タグを開かない `<` は文字として残すこと。
    ///
    /// HTML でも `<` の次が英字などでなければタグにならないので、`a < b > c`
    /// はそのまま表示される。タグとして落とすと本文が消える。
    #[test]
    fn a_less_than_that_opens_no_tag_is_kept() {
        assert_eq!(from_markdown("<div>a < b > c</div>"), "a &lt; b &gt; c");
    }

    /// ブロック要素を落としても前後がくっつかないこと。
    #[test]
    fn block_elements_are_separated() {
        assert_eq!(from_markdown("<pre>code</pre><p>after</p>"), "code\nafter");
        assert_eq!(from_markdown("<dl><dt>A</dt><dd>B</dd></dl>"), "A\nB");
    }

    /// 脚注の定義を落とさないこと。
    ///
    /// `ENABLE_FOOTNOTES` が無いと定義がリンク参照定義として扱われて消える。
    #[test]
    fn footnote_definitions_are_kept() {
        let out = from_markdown("本文[^1]\n\n[^1]: **定義**");

        assert_eq!(out, "本文[^1]\n\n[^1]: *定義*");
    }

    /// イベントを跨いで ``` ができないこと。
    ///
    /// 無効化は 1 つの文字列の中しか見ないので、`` ` `` が別のイベントに
    /// 分かれていると、繋がってから Slack のフェンスになる。開いたままの
    /// フェンスは以降の本文と Assignees まで飲み込む。
    #[test]
    fn fences_assembled_across_events_are_neutralized() {
        // escape した `` ` `` が HTML コメントで分かれる
        let split = from_markdown("\\`<!-- c -->\\`\\`");
        assert!(!split.contains("```"), "フェンスができている: {split:?}");

        // 数値参照はイベントごとに `` ` `` になる
        let refs = from_markdown("&#96;&#96;&#96;");
        assert!(!refs.contains("```"), "フェンスができている: {refs:?}");
    }

    /// 散文に書かれた ``` でもフェンスを開かせないこと。
    ///
    /// コードの中だけ見ていると取りこぼす。
    #[test]
    fn fences_in_prose_are_neutralized() {
        let out = from_markdown("見出しは ``` で囲みます\n\nafter");

        assert!(out.ends_with("after"), "本文が飲まれている: {out:?}");
        assert!(!out.contains("```"), "無効化されていない: {out:?}");
    }

    /// インラインコードの中の ``` でもフェンスを開かせないこと。
    #[test]
    fn inline_code_fences_are_neutralized() {
        let out = from_markdown("``` ``` ```\n\nafter");

        assert!(out.ends_with("after"), "本文が飲まれている: {out:?}");
    }

    /// `;` が無い数値参照も戻すこと。
    ///
    /// HTML では数字の並びが参照で、`;` は付いていれば取る。数字でない文字は
    /// 参照の外なのでそのまま残る。
    #[test]
    fn numeric_references_without_a_semicolon_are_decoded() {
        assert_eq!(from_markdown("<div>&#8230</div>"), "…");
        assert_eq!(from_markdown("<div>&#65foo;</div>"), "Afoo;");
    }

    /// 数値参照を HTML の規則で戻すこと。
    ///
    /// そのまま Unicode の番号として読むと、`&#0;` が NUL になって Slack に
    /// 送る本文に制御文字が入る。`&#128;` は Windows-1252 の `€`。
    ///
    /// `div` は HTML ブロックなので中身は markdown として解釈されず、
    /// こちらで戻すことになる。
    #[test]
    fn numeric_references_follow_the_html_rules() {
        assert_eq!(from_markdown("<div>&#0;x</div>"), "\u{fffd}x");
        assert_eq!(from_markdown("<div>&#128;</div>"), "€");
        assert_eq!(from_markdown("<div>&#xD800;</div>"), "\u{fffd}");
    }

    /// 文字参照を 1 段だけ戻すこと。
    ///
    /// `&#38;lt;` は HTML としては `&lt;` という文字列。2 段戻して `<` に
    /// してはいけない。
    #[test]
    fn character_references_are_decoded_once() {
        assert_eq!(from_markdown("<b>&#38;lt;</b>"), "&amp;lt;");
    }

    /// 内側の ``` を無効化して、外側の 1 組だけを残すこと。
    ///
    /// 内側をそのまま出すと、その ``` が Slack のフェンスとして読まれて
    /// 以降の本文まで飲み込む。
    #[test]
    fn inner_fences_are_neutralized() {
        let out = from_markdown("````\n```\ninner\n```\n````\n\nafter");

        assert!(out.ends_with("after"), "本文が飲まれている: {out:?}");
        assert!(out.contains("inner"), "中身が消えている: {out:?}");
        assert_eq!(out.matches("```").count(), 2, "フェンスの数が違う: {out:?}");
    }

    /// 参照で書かれた scheme のリンク先を落とさないこと。
    ///
    /// `&#104;ttps://…` は HTML では https。生のまま判定すると相対 URL と
    /// みなしてリンク先が消える。判定を戻した方で行っても、`javascript:` の
    /// ような scheme は絶対 URL ではないので通らない。
    #[test]
    fn an_encoded_scheme_keeps_its_destination() {
        let out = from_markdown(r#"<a href="&#104;ttps://example.com">label</a>"#);
        assert_eq!(out, "label (https://example.com)");

        let js = from_markdown(r#"<a href="&#106;avascript:alert(1)">label</a>"#);
        assert_eq!(js, "label", "scheme を通してしまった: {js:?}");
    }

    /// 別の属性値の中に属性の形があっても行き先にしないこと。
    ///
    /// 空白の後なら名前の切れ目としては正しいので、引用符の中を見ないと
    /// `title` に書いた URL がリンク先になってしまう。
    #[test]
    fn an_attribute_inside_a_quoted_value_is_not_an_attribute() {
        let out = from_markdown(r#"<a title="note href=https://evil.example.com">label</a>"#);

        assert!(
            !out.contains("evil.example.com"),
            "行き先を捏造した: {out:?}"
        );
    }

    /// 引用符の無い値の末尾の `/` を落とさないこと。
    ///
    /// HTML では値の一部で、閉じ方を表す `/` は空白で区切られている。
    /// 落とすと別の場所を指す。
    #[test]
    fn an_unquoted_value_keeps_its_trailing_slash() {
        let out = from_markdown("<a href=https://example.com/path/>label</a>");

        assert_eq!(out, "label (https://example.com/path/)");
    }

    /// 別の属性値から行き先を捏造しないこと。
    #[test]
    fn attributes_are_matched_on_a_boundary() {
        let out = from_markdown(r#"<a title="href=https://evil.example.com">label</a>"#);

        assert!(!out.contains("evil"), "捏造している: {out:?}");
        assert!(out.contains("label"), "{out:?}");
    }

    /// 属性値の文字参照を二重にエンコードしないこと。
    #[test]
    fn href_entities_are_decoded_once() {
        let out = from_markdown(r#"<a href="https://example.com/?a=1&amp;b=2">label</a>"#);

        assert!(out.contains("a=1&amp;b=2"), "二重になっている: {out:?}");
        assert!(!out.contains("amp;amp;"), "二重になっている: {out:?}");
    }

    /// `<code>` の中身は markdown として解釈されること。
    ///
    /// GitHub も inline HTML の中の markdown を処理するので、描画結果は
    /// 一致する。monospace のスタイルだけ失う。
    #[test]
    fn markdown_inside_raw_html_is_still_parsed() {
        assert_eq!(from_markdown("<code>*x*</code>"), "_x_");
    }

    /// 属性値の文字参照を二重に戻さないこと。
    #[test]
    fn image_attributes_are_decoded_once() {
        assert_eq!(from_markdown(r#"<img src="x" alt="&amp;lt;">"#), "&amp;lt;");
    }

    /// アンカーや相対パスをリンク記法にしないこと。
    ///
    /// `<#usage|Usage>` は mrkdwn では Slack のチャンネル参照になる。
    #[test]
    fn non_absolute_destinations_are_not_linked() {
        assert_eq!(from_markdown("[Usage](#usage)"), "Usage");
        assert_eq!(from_markdown("[doc](docs/README.md)"), "doc");
        assert_eq!(
            from_markdown("[mail](mailto:a@example.com)"),
            "<mailto:a@example.com|mail>"
        );
    }

    /// 生 HTML のリンクと画像の行き先を捨てないこと。
    #[test]
    fn raw_html_targets_are_kept() {
        let a = from_markdown(r#"<a href="https://example.com">label</a>"#);
        assert!(a.contains("label"), "{a:?}");
        assert!(a.contains("https://example.com"), "{a:?}");

        assert_eq!(
            from_markdown(r#"<img src="https://example.com/x.png" alt="shot">"#),
            "shot"
        );
        assert_eq!(
            from_markdown(r#"<img src="https://example.com/x.png">"#),
            "https://example.com/x.png"
        );
    }

    /// 複数行の HTML コメントの中身を漏らさないこと。
    ///
    /// pulldown-cmark は HTML ブロックを行ごとに別のイベントで渡すので、
    /// イベント単位で見ると途中の行が本文として出てしまう。
    #[test]
    fn multiline_html_comments_are_dropped() {
        let out = from_markdown("<!--\nprivate template guidance\n-->\n\ntext");

        assert!(!out.contains("guidance"), "コメントが漏れている: {out:?}");
        assert!(out.contains("text"), "本文が消えている: {out:?}");
    }

    /// details と summary の中身をくっつけないこと。
    #[test]
    fn details_and_summary_are_separated() {
        let out = from_markdown("<details><summary>Title</summary>Body</details>");

        assert!(!out.contains("TitleBody"), "くっついている: {out:?}");
        assert!(out.contains("Title"), "{out:?}");
        assert!(out.contains("Body"), "{out:?}");
    }

    /// 一部だけ太字の見出しを囲まないこと。
    ///
    /// 囲むと `**重要* の話*` のように区切りが重なって壊れる。
    #[test]
    fn headings_with_partial_bold_are_not_wrapped() {
        assert_eq!(
            from_markdown("# **Important** details"),
            "*Important* details"
        );
    }

    /// 表のセルを落として語をくっつけないこと。
    #[test]
    fn html_table_cells_are_separated() {
        assert_eq!(
            from_markdown("<table><tr><td>A</td><td>B</td></tr></table>"),
            "A B"
        );
    }

    /// コードブロックの末尾の空行を消さないこと。
    ///
    /// 末尾の空白もコードの一部。閉じフェンスの直前の改行だけ落とす。
    #[test]
    fn code_blocks_keep_their_trailing_blank_lines() {
        let out = from_markdown("```\ncode\n\n```");

        assert!(out.contains("code\n\n```"), "空行が消えている: {out:?}");
    }

    /// 数値文字参照を戻すこと。
    #[test]
    fn numeric_character_references_are_decoded() {
        assert_eq!(from_markdown("<b>&#8230;</b>"), "\u{2026}");
        assert_eq!(from_markdown("<b>&#x2026;</b>"), "\u{2026}");
        // 参照になっていないものはそのまま
        assert_eq!(from_markdown("<b>&#;</b>"), "&amp;#;");
    }

    /// HTML コメントは中身ごと落とすこと。
    ///
    /// issue テンプレートの説明文が通知に出ると邪魔になる。
    #[test]
    fn html_comments_are_dropped() {
        assert!(from_markdown("<!-- 説明 -->").is_empty());
        assert_eq!(from_markdown("<!-- 説明 -->text"), "text");
    }

    /// 段落以外で始まる項目でも印と中身が離れないこと。
    ///
    /// 見出しやコードブロックが項目の先頭に来ることもある。
    #[test]
    fn list_items_starting_with_a_block_keep_their_markers() {
        assert_eq!(from_markdown("- # title"), "• *title*");

        let out = from_markdown("- ```\ncode\n```");
        assert!(!out.starts_with("• \n"), "印だけの行ができている: {out:?}");
    }

    /// 空行を含むリストでも印と本文が離れないこと。
    ///
    /// 項目の中に段落が来ると、段落として空行を入れてしまい `• ` だけの行に
    /// なってしまう。
    #[test]
    fn loose_lists_keep_their_markers() {
        assert_eq!(from_markdown("- a\n\n- b"), "• a\n\n• b");
    }

    /// 表の見出しが既に太字なら二重にしないこと。
    #[test]
    fn already_bold_table_headers_are_not_wrapped_again() {
        assert_eq!(
            from_markdown("| **Name** | x |\n| --- | --- |\n| 1 | 2 |"),
            "*Name* | *x*\n1 | 2"
        );
    }

    /// 装飾の無い本文は変えないこと。通知の大半はこれ。
    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(
            from_markdown("LGTM! @sksat ありがとう"),
            "LGTM! @sksat ありがとう"
        );
    }

    /// 段落の間は空行のまま、行内の改行はそのままにすること。
    #[test]
    fn line_structure_is_kept() {
        assert_eq!(from_markdown("one\n\ntwo"), "one\n\ntwo");
        assert_eq!(from_markdown("one\ntwo"), "one\ntwo");
    }

    /// 前後の空白は落とすこと。空の本文で `no_text` にならないようにする。
    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(from_markdown("\n\n  body  \n\n"), "body");
        assert!(from_markdown("").is_empty());
        assert!(from_markdown("   \n  ").is_empty());
    }
}
