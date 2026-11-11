//! The api stages' nesting shapes (findings/conformance/work/api/depth/shapes.cjs builds the
//! same ones in node): `api_depth STAGE SHAPE N [STACK_KB]` runs STAGE (format, reactStatic,
//! react) over SHAPE nested N deep on a thread with STACK_KB of stack (default 2048), and
//! prints `ok` or the error. A stack overflow aborts the process.

use markdoc::ast::{Function, Node};
use markdoc::next::formatter::{self, FormatOptions};
use markdoc::next::index;
use markdoc::next::renderers::{react, react_static};
use markdoc::next::schema::value::{TagValue, Value};
use markdoc::types::{JsObject, NodeType, Scalar};

fn node(t: NodeType, attributes: Vec<(&str, Scalar)>, children: Vec<Node>) -> Node {
    Node::new(
        t,
        attributes.into_iter().map(|(k, v)| (k.to_string(), v)),
        children,
        None,
    )
}

fn text(s: &str) -> Node {
    node(
        NodeType::Text,
        vec![("content", Scalar::String(s.into()))],
        vec![],
    )
}

fn para(child: Node) -> Node {
    node(
        NodeType::Paragraph,
        vec![],
        vec![node(NodeType::Inline, vec![], vec![child])],
    )
}

fn doc(child: Node) -> Node {
    node(NodeType::Document, vec![], vec![child])
}

fn tag(name: &str, attributes: Vec<(&str, Scalar)>, children: Vec<Node>, inline: bool) -> Node {
    let mut t = node(NodeType::Tag, attributes, children);
    t.tag = Some(name.into());
    t.inline = inline;
    t
}

fn nest<T>(n: usize, leaf: T, wrap: impl Fn(T) -> T) -> T {
    let mut x = leaf;
    for _ in 0..n {
        x = wrap(x);
    }
    x
}

fn format_shape(shape: &str, n: usize) -> Node {
    let s = |m: &str| Scalar::String(m.into());
    let same = |t: NodeType, attributes: Vec<(&str, Scalar)>| {
        doc(nest(n, para(text("x")), |x| {
            node(t.clone(), attributes.clone(), vec![x])
        }))
    };
    match shape {
        "documents" => same(NodeType::Document, vec![]),
        "headings" => same(NodeType::Heading, vec![("level", Scalar::Number(1.0))]),
        "paragraphs" => same(NodeType::Paragraph, vec![]),
        "inlines" => same(NodeType::Inline, vec![]),
        "items" => same(NodeType::Item, vec![]),
        "tbodies" => same(NodeType::Tbody, vec![]),
        "tds" => same(NodeType::Td, vec![]),
        "ths" => same(NodeType::Th, vec![]),
        "theads" => same(NodeType::Thead, vec![]),
        "trs" => doc(node(
            NodeType::Table,
            vec![],
            vec![node(
                NodeType::Tbody,
                vec![],
                vec![nest(n, para(text("x")), |x| {
                    node(NodeType::Tr, vec![], vec![x])
                })],
            )],
        )),
        "s" => doc(para(nest(n, text("x"), |x| {
            node(NodeType::Strikethrough, vec![], vec![x])
        }))),
        "tables" => doc(nest(n, para(text("x")), |x| {
            let td = node(NodeType::Td, vec![], vec![x]);
            let tr = node(NodeType::Tr, vec![], vec![td]);
            let tbody = node(NodeType::Tbody, vec![], vec![tr]);
            tag(
                "table",
                vec![],
                vec![node(NodeType::Table, vec![], vec![tbody])],
                false,
            )
        })),
        "tags" => doc(nest(n, para(text("x")), |x| {
            tag("a", vec![], vec![x], false)
        })),
        "inline_tags" => doc(para(nest(n, text("x"), |x| {
            tag("a", vec![], vec![x], true)
        }))),
        "blockquotes" => doc(nest(n, para(text("x")), |x| {
            node(NodeType::Blockquote, vec![], vec![x])
        })),
        "lists" => doc(nest(n, para(text("x")), |x| {
            node(
                NodeType::List,
                vec![("ordered", Scalar::Boolean(false)), ("marker", s("-"))],
                vec![node(NodeType::Item, vec![], vec![x])],
            )
        })),
        "em" => doc(para(nest(n, text("x"), |x| {
            node(NodeType::Em, vec![("marker", s("*"))], vec![x])
        }))),
        "strong" => doc(para(nest(n, text("x"), |x| {
            node(NodeType::Strong, vec![("marker", s("**"))], vec![x])
        }))),
        "links" => doc(para(nest(n, text("x"), |x| {
            node(NodeType::Link, vec![("href", s("u"))], vec![x])
        }))),
        "arrays" => doc(tag(
            "a",
            vec![(
                "b",
                nest(n, Scalar::Array(vec![]), |x| Scalar::Array(vec![x])),
            )],
            vec![],
            false,
        )),
        "objects" => doc(tag(
            "a",
            vec![(
                "b",
                nest(n, Scalar::Number(1.0), |x| {
                    Scalar::Object([("a".to_string(), x)].into_iter().collect())
                }),
            )],
            vec![],
            false,
        )),
        "calls" => doc(tag(
            "a",
            vec![(
                "b",
                nest(
                    n,
                    Scalar::Function(Function::new("f".into(), Default::default())),
                    |x| {
                        Scalar::Function(Function::new(
                            "f".into(),
                            [("0".to_string(), x)].into_iter().collect(),
                        ))
                    },
                ),
            )],
            vec![],
            false,
        )),
        other => panic!("no format shape {other}"),
    }
}

fn react_shape(shape: &str, n: usize) -> Value {
    let tag = |attributes: Vec<(&str, Value)>, children: Vec<Value>| {
        Value::from(TagValue::new(
            "div",
            attributes
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<JsObject<Value>>(),
            children,
        ))
    };
    match shape {
        "tags" => nest(n, Value::from("x"), |x| tag(vec![], vec![x])),
        "arrays" => nest(n, Value::from("x"), |x| Value::array(vec![x])),
        "attr_arrays" => tag(
            vec![("a", nest(n, Value::from(1.0), |x| Value::array(vec![x])))],
            vec![],
        ),
        "attr_objects" => tag(
            vec![(
                "a",
                nest(n, Value::from(1.0), |x| {
                    Value::object(
                        [("a".to_string(), x)]
                            .into_iter()
                            .collect::<JsObject<Value>>(),
                    )
                }),
            )],
            vec![],
        ),
        "attr_tags" => nest(n, Value::from(1.0), |x| tag(vec![("a", x)], vec![])),
        other => panic!("no react shape {other}"),
    }
}

struct Null;

impl react::React for Null {
    type Element = ();
    type Component = ();
    fn create_element(
        &mut self,
        _: react::ElementType<'_, ()>,
        _: Option<react::Prop<'_, ()>>,
        _: Vec<react::Rendered<'_, ()>>,
    ) {
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (stage, shape, n) = (args[1].clone(), args[2].clone(), args[3].parse().unwrap());
    let stack_kb: usize = args.get(4).map_or(2048, |s| s.parse().unwrap());
    let out = std::thread::Builder::new()
        .stack_size(stack_kb * 1024)
        .spawn(move || match stage.as_str() {
            "format" => {
                let ast = format_shape(&shape, n);
                let r = formatter::format(&ast, &FormatOptions::default()).map(|_| ());
                std::mem::forget(ast);
                r
            }
            "reactStatic" => {
                let tree = react_shape(&shape, n);
                let r = react_static::render(&tree, &Default::default()).map(|_| ());
                std::mem::forget(tree);
                r
            }
            "react" => {
                let tree = react_shape(&shape, n);
                let r = react::render(&tree, &mut Null, &Default::default()).map(|_| ());
                std::mem::forget(tree);
                r
            }
            "getAstValues" => {
                let value = match shape.as_str() {
                    "arrays" => nest(n, Scalar::Number(1.0), |x| Scalar::Array(vec![x])),
                    "objects" => nest(n, Scalar::Number(1.0), |x| {
                        Scalar::Object([("a".to_string(), x)].into_iter().collect())
                    }),
                    other => panic!("no value shape {other}"),
                };
                let r = markdoc::next::ast::get_ast_values(&value).map(|_| ());
                std::mem::forget(value);
                r
            }
            other => panic!("no stage {other}"),
        })
        .unwrap()
        .join()
        .unwrap();
    match out {
        Ok(()) => println!("ok"),
        Err(e) => println!("{}", index::thrown(&e)),
    }
}
