//! `docs/loop.html` explains the disciplining loop; `docs/firmware/loop.md`
//! is the evidence for it.  Each value the page's tables state must be
//! one the evidence states, so the two cannot drift apart unnoticed.

use std::fs;

/// The repository's root, from this crate's directory.
fn root() -> String {
    format!("{}/../..", env!("CARGO_MANIFEST_DIR"))
}

/// The text of every `<td>` in `html`'s tables, by row.
fn rows(html: &str) -> Vec<Vec<String>> {
    html.split("<tr>")
        .skip(1)
        .map(|row| {
            row.split("<td")
                .skip(1)
                .map(|cell| {
                    let cell = cell.split_once('>').map_or(cell, |(_, rest)| rest);
                    cell.split("</td>").next().unwrap_or_default().to_owned()
                })
                .collect()
        })
        .collect()
}

/// Each value written `N × 10ⁿ`, with its sign, in `text`.
fn powers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices(" × 10") {
        let before = &text[..at];
        let start = before
            .rfind(|letter: char| !(letter.is_ascii_digit() || letter == '.'))
            .map_or(0, |index| {
                let sign = before[index..].chars().next().unwrap_or(' ');
                if sign == '−' || sign == '+' {
                    index
                } else {
                    index + sign.len_utf8()
                }
            });
        let after = &text[at + " × 10".len()..];
        let exponent: String = after
            .chars()
            .take_while(|letter| "⁻⁰¹²³⁴⁵⁶⁷⁸⁹".contains(*letter))
            .collect();
        found.push(format!("{} × 10{exponent}", &text[start..at]));
    }
    found
}

/// Each whole number of seconds, `N s`, in `text`.
fn seconds(text: &str) -> Vec<String> {
    text.match_indices(" s")
        .filter_map(|(at, _)| {
            let digits: String = text[..at]
                .chars()
                .rev()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then(|| format!("{} s", digits.chars().rev().collect::<String>()))
        })
        .collect()
}

#[test]
fn the_loop_page_states_only_values_its_evidence_states() {
    let page = fs::read_to_string(format!("{}/docs/loop.html", root())).expect("the page");
    let evidence =
        fs::read_to_string(format!("{}/docs/firmware/loop.md", root())).expect("the evidence");
    let by_model = &page[page.find("<h3>By model</h3>").expect("the by-model table")..];
    let by_model = &by_model[..by_model.find("</table>").expect("its end")];
    let mut values: Vec<String> = rows(by_model)
        .iter()
        .flat_map(|row| row.iter().skip(1).take(2))
        .flat_map(|cell| seconds(cell).into_iter().chain(powers(cell)))
        .collect();
    values.extend(
        rows(&page[..page.find("<h3>By model</h3>").expect("the by-model table")])
            .iter()
            .flatten()
            .flat_map(|cell| powers(cell)),
    );
    assert!(!values.is_empty(), "no values found on the page");
    for value in values {
        assert!(
            evidence.contains(&value),
            "loop.html states {value}, which loop.md does not"
        );
    }
}
