use pulldown_cmark::{Options, Parser, html};

#[test]
fn nested_image_wikilinks_keep_the_deepest_embed() {
    for (markdown, expected) in [
        (
            "![[![[dog.png]]]]",
            "<p>![[<img src=\"dog.png\" alt=\"dog.png\" />]]</p>\n",
        ),
        (
            "![[cat.png|![[dog.png]]]]",
            "<p>![[cat.png|<img src=\"dog.png\" alt=\"dog.png\" />]]</p>\n",
        ),
    ] {
        let mut rendered = String::new();
        html::push_html(
            &mut rendered,
            Parser::new_ext(markdown, Options::ENABLE_WIKILINKS),
        );
        assert_eq!(rendered, expected);
    }
}
