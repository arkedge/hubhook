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
//! - markdown で escape した記号 (`\*literal\*`) は escape が外れた状態で
//!   渡ってくるため、Slack が装飾として解釈する
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

    if name == "br" {
        return Some('\n');
    }
    if !tag.starts_with('/') {
        return None;
    }

    match name.as_str() {
        "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" => {
            Some('\n')
        }
        // <details><summary>Title</summary>Body</details> が TitleBody に
        // ならないようにする
        "summary" | "details" => Some('\n'),
        "td" | "th" => Some(' '),
        _ => None,
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
            let end = digits.find(';')?;
            if end == 0 {
                return None;
            }
            let code = u32::from_str_radix(&digits[..end], radix).ok()?;
            Some((char::from_u32(code)?, &digits[end + 1..]))
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
fn neutralize_fences(text: &str) -> String {
    text.replace("```", "`\u{200b}`\u{200b}`")
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
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;

    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        // 属性名の切れ目を確かめる。`"` を境界に含めると、別の属性値の中の
        // 文字列 (`<a title="href=...">`) から行き先を捏造してしまう。
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = &tag[at + name.len()..];
        let rest_trimmed = rest.trim_start();

        if before_ok && rest_trimmed.starts_with('=') {
            let value = rest_trimmed[1..].trim_start();
            let quoted = value.strip_prefix('"').or_else(|| value.strip_prefix('\''));
            return match quoted {
                Some(v) => {
                    let q = value.as_bytes()[0] as char;
                    v.find(q).map(|e| v[..e].to_string())
                }
                // 引用符なしの値は空白まで
                None => Some(
                    value
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .trim_end_matches('/')
                        .to_string(),
                ),
            };
        }
        from = at + name.len();
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
    at_item_start: bool,
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
            at_item_start: false,
        }
    }

    fn push(&mut self, s: &str) {
        self.bufs.last_mut().expect("書き込み先が無い").push_str(s);
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

            if html[i..].starts_with("<!--") {
                self.in_html_comment = true;
                i += 4;
                continue;
            }

            if bytes[i] == b'<' {
                let mut j = i + 1;
                let mut quote: Option<u8> = None;

                while j < bytes.len() {
                    let c = bytes[j];
                    // `"` `'` `>` は ASCII なので UTF-8 の後続バイトと衝突しない
                    match quote {
                        Some(q) if c == q => quote = None,
                        Some(_) => {}
                        None if c == b'"' || c == b'\'' => quote = Some(c),
                        None if c == b'>' => break,
                        None => {}
                    }
                    j += 1;
                }

                if j >= bytes.len() {
                    // 閉じていないので、以降はタグの途中とみなして捨てる
                    break;
                }

                let tag = &html[i + 1..j];

                // 区切りを意味するタグは、落とすと語がくっつく (`a<br>b` -> `ab`)
                if let Some(sep) = tag_separator(tag) {
                    out.push(sep);
                }

                // タグを落とすと行き先まで消える。`<img>` は中に文字が無いので
                // alt か src を出さないと本文が空になる。
                out.push_str(&self.html_target(tag));

                i = j + 1;
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
                self.pending_href = attr(tag, "href").filter(|u| is_absolute(u));
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
    /// ただしリスト項目の印を書いた直後は入れない。入れると印と中身が離れて
    /// `• ` だけの行ができる。段落に限らず、見出しやコードブロックが項目の
    /// 先頭に来ることもある。
    fn block_start(&mut self) {
        if self.at_item_start {
            self.at_item_start = false;
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

    let mut r = Renderer::new();

    for event in Parser::new_ext(md, opts) {
        match event {
            Event::Start(tag) => start(&mut r, tag),
            Event::End(tag) => end(&mut r, tag),
            Event::Text(t) => {
                let escaped = escape(&t);
                r.push(&escaped);
            }
            Event::Code(t) => {
                let escaped = escape(&t);

                // mrkdwn のコード span は ` で囲む以外に書き方が無いので、
                // 中に ` が残っていると区切りが壊れる。素のテキストで出す。
                if escaped.contains('`') {
                    r.push(&escaped);
                } else {
                    r.push(&format!("`{escaped}`"));
                }
            }
            Event::SoftBreak | Event::HardBreak => r.push("\n"),
            // Slack に水平線は無い。段落の切れ目としてだけ扱う
            Event::Rule => r.block_start(),
            Event::TaskListMarker(done) => r.push(if done { "☑ " } else { "☐ " }),
            // タグは落とすが中の文字は残す。pulldown-cmark は HTML ブロックを
            // まとめて 1 つのイベントで渡すので、丸ごと捨てると本文が消える
            Event::Html(h) | Event::InlineHtml(h) => {
                let visible = r.html_text(&h);
                if !visible.is_empty() {
                    let escaped = escape(&visible);
                    r.push(&escaped);
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
        Tag::Paragraph => r.block_start(),
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
        }
        Tag::BlockQuote(kind) => {
            r.block_start();
            // 中身を組み立ててから各行に "> " を付ける
            r.open();

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
                r.at_item_start = false;
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
            r.at_item_start = true;
        }
        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
            r.links.push(dest_url.to_string());
            r.open();
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
            let inner = r.close();
            // 引用記法の ">" は生で置く。`&gt;` にすると Slack は引用として
            // 解釈せず、リテラルの ">" を表示する
            let quoted = inner
                .trim_end()
                .lines()
                .map(|l| {
                    if l.is_empty() {
                        ">".to_string()
                    } else {
                        format!("> {l}")
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
        TagEnd::Item => r.newline(),
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
        TagEnd::Table => {
            if let Some(t) = r.table.take() {
                let rendered = render_table(&t.rows);
                r.push(&rendered);
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

    /// 文字参照を 1 段だけ戻すこと。
    ///
    /// `&#38;lt;` は HTML としては `&lt;` という文字列。2 段戻して `<` に
    /// してはいけない。
    #[test]
    fn character_references_are_decoded_once() {
        assert_eq!(from_markdown("<b>&#38;lt;</b>"), "&amp;lt;");
    }

    /// 内側の ``` でフェンスを開かせないこと。
    ///
    /// 囲まずに出すと、その ``` が Slack のフェンスとして読まれて以降の
    /// 本文まで飲み込む。
    #[test]
    fn inner_fences_are_neutralized() {
        let out = from_markdown("````\n```\ninner\n```\n````\n\nafter");

        assert!(out.ends_with("after"), "本文が飲まれている: {out:?}");
        assert!(out.contains("inner"), "中身が消えている: {out:?}");
        assert_eq!(out.matches("```").count(), 2, "フェンスの数が違う: {out:?}");
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

    /// コードブロックの中に ``` があるときは囲まないこと。
    ///
    /// Slack のコードブロックは ``` 固定で長さを変えられないので、囲むと
    /// 中の ``` で途中で閉じて、以降の装飾まで崩れる。
    #[test]
    fn code_blocks_containing_a_fence_are_not_fenced() {
        let out = from_markdown("````\n```\ninner\n```\n````");

        assert!(!out.starts_with("```\n```"), "二重に囲んでいる: {out:?}");
        assert!(out.contains("inner"), "中身が消えている: {out:?}");
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
