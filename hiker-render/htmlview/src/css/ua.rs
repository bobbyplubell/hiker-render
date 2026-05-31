//! User-agent stylesheet.
//!
//! Ported (nearly verbatim) from litehtml's `master_css.h`: default `display`
//! values for structural/table/list/form tags, `head/script/style/title/meta/link
//! { display:none }`, default block margins, `a:link` blue+underline, table
//! defaults (`border-collapse:separate; border-spacing:2px`, cell padding, the
//! `[border]` attribute rules), `pre/code` monospace + `white-space:pre`, `hr`,
//! `sub/sup`, list padding, etc.
//!
//! Theme handling: the cascade picks the base text/background colors per
//! [`crate::Theme`] (see `cascade.rs`), and additionally appends [`UA_CSS_DARK`]
//! after [`UA_CSS`] (still as UA-origin rules) when the theme is `Dark`. The dark
//! block only flips the small set of hard-coded colors the UA sheet sets (link
//! color, table border colors) so they remain legible on a dark background.

/// The base UA stylesheet (theme-independent). Applied for every theme.
pub const UA_CSS: &str = r#"
html { display: block; }
head, meta, title, link, style, script { display: none; }
base, param, noscript, template { display: none; }

body { display: block; margin: 8px; }

p { display: block; margin-top: 1em; margin-bottom: 1em; }

b, strong { display: inline; font-weight: bold; }
i, em, cite, var, dfn, address { display: inline; font-style: italic; }
ins, u { text-decoration: underline; }
del, s, strike { text-decoration: line-through; }
small { font-size: smaller; }
big { font-size: larger; }
mark { background-color: yellow; color: black; }

center { text-align: center; display: block; }

a:link { text-decoration: underline; color: #0645ad; }

h1, h2, h3, h4, h5, h6, div { display: block; }
h1 { font-weight: bold; margin-top: 0.67em; margin-bottom: 0.67em; font-size: 2em; }
h2 { font-weight: bold; margin-top: 0.83em; margin-bottom: 0.83em; font-size: 1.5em; }
h3 { font-weight: bold; margin-top: 1em; margin-bottom: 1em; font-size: 1.17em; }
h4 { font-weight: bold; margin-top: 1.33em; margin-bottom: 1.33em; }
h5 { font-weight: bold; margin-top: 1.67em; margin-bottom: 1.67em; font-size: 0.83em; }
h6 { font-weight: bold; margin-top: 2.33em; margin-bottom: 2.33em; font-size: 0.67em; }

br { display: inline-block; }
br[clear="all"] { clear: both; }
br[clear="left"] { clear: left; }
br[clear="right"] { clear: right; }

span { display: inline; }
img { display: inline-block; }
img[align="right"] { float: right; }
img[align="left"] { float: left; }

hr {
    display: block;
    margin-top: 0.5em;
    margin-bottom: 0.5em;
    margin-left: auto;
    margin-right: auto;
    border-style: inset;
    border-width: 1px;
    color: gray;
}

/***************** TABLES ********************/
table {
    display: table;
    border-collapse: separate;
    border-spacing: 2px;
    border-top-color: gray;
    border-left-color: gray;
    border-bottom-color: black;
    border-right-color: black;
}
tbody, tfoot, thead { display: table-row-group; vertical-align: middle; }
thead { display: table-header-group; }
tfoot { display: table-footer-group; }
tr { display: table-row; vertical-align: inherit; border-color: inherit; }
td, th { display: table-cell; vertical-align: inherit; border-width: 1px; padding: 1px; }
th { font-weight: bold; text-align: center; }
table[border] { border-style: solid; }
table[border] td, table[border] th { border-style: solid; }
table[align="left"] { float: left; }
table[align="right"] { float: right; }
table[align="center"] { margin-left: auto; margin-right: auto; }
caption { display: table-caption; text-align: center; }
col { display: table-column; }
colgroup { display: table-column-group; }
td[nowrap], th[nowrap] { white-space: nowrap; }

/***************** MONOSPACE ********************/
tt, code, kbd, samp { font-family: monospace; }
pre, xmp, plaintext, listing {
    display: block;
    font-family: monospace;
    white-space: pre;
    margin-top: 1em;
    margin-bottom: 1em;
}

/***************** LISTS ********************/
ul, menu, dir {
    display: block;
    list-style-type: disc;
    margin-top: 1em;
    margin-bottom: 1em;
    padding-left: 40px;
}
ol {
    display: block;
    list-style-type: decimal;
    margin-top: 1em;
    margin-bottom: 1em;
    padding-left: 40px;
}
li { display: list-item; }
ul ul, ol ul { list-style-type: circle; }
ol ol ul, ol ul ul, ul ol ul, ul ul ul { list-style-type: square; }
ol ul, ul ol, ul ul, ol ol { margin-top: 0; margin-bottom: 0; }
dl { display: block; margin-top: 1em; margin-bottom: 1em; }
dt { display: block; }
dd { display: block; margin-left: 40px; }

blockquote {
    display: block;
    margin-top: 1em;
    margin-bottom: 1em;
    margin-left: 40px;
    margin-right: 40px;
}

/*********** FORM ELEMENTS ************/
form { display: block; margin-top: 0; }
fieldset { display: block; }
legend { display: block; }
label { display: inline; }
option { display: none; }
input, textarea, select, button {
    margin: 0;
    line-height: normal;
    display: inline-block;
}
input[type="hidden"] { display: none; }

/*********** HTML5 SECTIONING ************/
article, aside, footer, header, hgroup, nav, section, main, figcaption {
    display: block;
}
details, summary { display: block; }

figure {
    display: block;
    margin-top: 1em;
    margin-bottom: 1em;
    margin-left: 40px;
    margin-right: 40px;
}

sub { vertical-align: sub; font-size: smaller; }
sup { vertical-align: super; font-size: smaller; }
"#;

/// Theme override block appended (as UA-origin rules) after [`UA_CSS`] when the
/// theme is `Dark`. Flips the hard-coded UA colors so they stay legible on a dark
/// background. The overall page text/background defaults come from the cascade's
/// per-theme base style (see `cascade.rs`); this only patches UA-set colors.
pub const UA_CSS_DARK: &str = r#"
a:link { color: #6db3f2; }
table {
    border-top-color: #777;
    border-left-color: #777;
    border-bottom-color: #aaa;
    border-right-color: #aaa;
}
mark { background-color: #665c00; color: #eee; }
hr { color: #777; }
"#;
