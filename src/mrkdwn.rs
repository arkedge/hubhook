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
//! | 表 | コードブロックに入れて桁を揃える |

use pulldown_cmark::{BlockQuoteKind, Event, Options, Parser, Tag, TagEnd};

/// Slack のテキストで意味を持つ 3 文字。
///
/// <https://docs.slack.dev/messaging/formatting-message-text>
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// リンクの URL 側。`<` `>` は区切りなので入れられないが、`&` は escape する。
fn escape_url(url: &str) -> String {
    url.replace('&', "&amp;")
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
                r.push(&format!("`{escaped}`"));
            }
            Event::SoftBreak | Event::HardBreak => r.push("\n"),
            // Slack に水平線は無い。段落の切れ目としてだけ扱う
            Event::Rule => r.blank_line(),
            Event::TaskListMarker(done) => r.push(if done { "☑ " } else { "☐ " }),
            // 生の HTML は落とす。`<!-- -->` や `<details>` がそのまま出ると読めない
            Event::Html(_) | Event::InlineHtml(_) => {}
            _ => {}
        }
    }

    let out = r.close();
    out.trim().to_string()
}

fn start(r: &mut Renderer, tag: Tag) {
    match tag {
        Tag::Paragraph => r.blank_line(),
        // 見出しが無いので太字で代用する
        Tag::Heading { .. } => {
            r.blank_line();
            r.push("*");
        }
        Tag::Strong => r.push("*"),
        Tag::Emphasis => r.push("_"),
        Tag::Strikethrough => r.push("~"),
        Tag::CodeBlock(_) => {
            r.blank_line();
            r.push("```\n");
        }
        Tag::BlockQuote(kind) => {
            r.blank_line();
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
        }
        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
            r.links.push(dest_url.to_string());
            r.open();
        }
        Tag::Table(_) => {
            r.blank_line();
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
            r.push("*");
            r.blank_line();
        }
        TagEnd::Strong => r.push("*"),
        TagEnd::Emphasis => r.push("_"),
        TagEnd::Strikethrough => r.push("~"),
        TagEnd::CodeBlock => {
            r.newline();
            r.push("```");
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
        TagEnd::Link | TagEnd::Image => {
            let label = r.close();
            let url = r.links.pop().unwrap_or_default();
            // ラベルが無いリンクは URL だけ出す。`<url|>` は空ラベルになる
            if label.trim().is_empty() {
                r.push(&format!("<{}>", escape_url(&url)));
            } else {
                r.push(&format!("<{}|{}>", escape_url(&url), label));
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

/// 表はコードブロックに入れる。
///
/// mrkdwn に表は無く、`|` を並べただけでは桁が揃わずに読めない。
/// 等幅で描画される場所に入れて、幅を揃える。
fn render_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let line = |row: &Vec<String>| {
        let cells: Vec<String> = (0..cols)
            .map(|i| {
                let cell = row.get(i).map(String::as_str).unwrap_or("");
                let pad = widths[i].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect();
        format!("| {} |", cells.join(" | "))
    };

    let mut out = String::from("```\n");
    out.push_str(&line(&rows[0]));
    out.push('\n');

    // 見出しと本体の区切り
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&format!("| {} |\n", sep.join(" | ")));

    for row in &rows[1..] {
        out.push_str(&line(row));
        out.push('\n');
    }
    out.push_str("```");
    out
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

    /// 表は等幅ブロックに入れて桁を揃えること。
    ///
    /// mrkdwn に表は無く、`|` を並べただけでは桁が揃わずに読めない。
    #[test]
    fn tables_are_aligned_in_a_code_block() {
        assert_eq!(
            from_markdown("| a | bb |\n| --- | --- |\n| 1 | 2 |"),
            "```\n| a | bb |\n| - | -- |\n| 1 | 2  |\n```"
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
