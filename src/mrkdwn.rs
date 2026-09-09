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

use pulldown_cmark::{BlockQuoteKind, Event, Options, Parser, Tag, TagEnd};

/// Slack のテキストで意味を持つ 3 文字。
///
/// <https://docs.slack.dev/messaging/formatting-message-text>
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
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

/// HTML のタグとコメントを落として、見える文字だけ返す。
///
/// issue のテンプレートは `<!-- 説明 -->` や `<details>` を含む。タグを
/// そのまま出すと読めないが、イベントごと捨てると中の文字まで消える。
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;

    loop {
        // コメントは中身ごと落とす
        if let Some(i) = rest.find("<!--") {
            out.push_str(&rest[..i]);
            match rest[i..].find("-->") {
                Some(j) => rest = &rest[i + j + 3..],
                None => return out.trim().to_string(),
            }
            continue;
        }

        match rest.find('<') {
            None => {
                out.push_str(rest);
                return out.trim().to_string();
            }
            Some(i) => {
                out.push_str(&rest[..i]);
                match rest[i..].find('>') {
                    Some(j) => rest = &rest[i + j + 1..],
                    // 閉じていないので、以降はタグの途中とみなして捨てる
                    None => return out.trim().to_string(),
                }
            }
        }
    }
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
                // 中に ` があると区切りが壊れる。素のテキストとして出す。
                if escaped.contains('`') {
                    r.push(&escaped);
                } else {
                    r.push(&format!("`{escaped}`"));
                }
            }
            Event::SoftBreak | Event::HardBreak => r.push("\n"),
            // Slack に水平線は無い。段落の切れ目としてだけ扱う
            Event::Rule => r.blank_line(),
            Event::TaskListMarker(done) => r.push(if done { "☑ " } else { "☐ " }),
            // タグは落とすが中の文字は残す。pulldown-cmark は HTML ブロックを
            // まとめて 1 つのイベントで渡すので、丸ごと捨てると本文が消える
            Event::Html(h) | Event::InlineHtml(h) => {
                let visible = strip_tags(&h);
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
        // 項目の印の直後の段落は、印と本文を離さないように空行を入れない
        Tag::Paragraph => {
            if r.at_item_start {
                r.at_item_start = false;
            } else {
                r.blank_line();
            }
        }
        // 見出しが無いので太字で代用する。中身を読んでから決めるので開く
        Tag::Heading { .. } => {
            r.blank_line();
            r.open();
        }
        Tag::Strong => r.push("*"),
        Tag::Emphasis => r.push("_"),
        Tag::Strikethrough => r.push("~"),
        // 中身を読んでから囲むかを決めるので開く
        Tag::CodeBlock(_) => {
            r.blank_line();
            r.open();
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
            r.at_item_start = true;
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
            let inner = r.close();
            let inner = inner.trim();

            // 中身が既に太字なら二重にしない。`# **重要**` が `**重要**` に
            // なると、mrkdwn では太字として解釈されない。
            //
            // `*a* *b*` のように太字が 2 つ並ぶ場合も囲まないままにする。
            // 見出し全体の太字は失うが、記法が壊れるより良い。
            let already_bold = inner.len() > 1 && inner.starts_with('*') && inner.ends_with('*');
            if inner.is_empty() {
                // 空の見出しで `**` を作らない
            } else if already_bold {
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
            let code = code.trim_end();

            // Slack のコードブロックは ``` 固定で長さを変えられない。中に ```
            // があると途中で閉じて、以降の装飾まで崩れる。囲むのを諦める。
            if code.contains("```") {
                r.push(code);
            } else {
                r.push(&format!("```\n{code}\n```"));
            }
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
                // 中身が既に太字なら二重にしない (見出しと同じ理由)
                let already_bold = c.len() > 1 && c.starts_with('*') && c.ends_with('*');
                if i == 0 && !c.is_empty() && !already_bold {
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

    /// HTML コメントは中身ごと落とすこと。
    ///
    /// issue テンプレートの説明文が通知に出ると邪魔になる。
    #[test]
    fn html_comments_are_dropped() {
        assert!(from_markdown("<!-- 説明 -->").is_empty());
        assert_eq!(from_markdown("<!-- 説明 -->text"), "text");
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
