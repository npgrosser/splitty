//! The code side: the modules of every library and binary target, found by following `mod`
//! declarations from the crate root, and every path by which a module names a module of the
//! workspace, marked as test or production code.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Spacing, TokenStream, TokenTree};
use syn::ext::IdentExt;
use syn::visit::Visit;

/// A module path such as `splitty_core::mux::reducer`, segment by segment.
pub type ModulePath = Vec<String>;

pub fn display(path: &[String]) -> String {
    path.join("::")
}

/// A library or binary target: its crate name (`splitty_core`, `splitty`) and its root file.
#[derive(Debug, Clone)]
pub struct Target {
    pub crate_name: String,
    pub root: PathBuf,
    /// Whether other crates can name it (`splitty_core::…`), as for a library.
    pub library: bool,
}

/// A source file of a target's module tree.
pub struct File {
    pub module: ModulePath,
    /// The target's root file (`lib.rs`, `main.rs`).
    pub crate_root: bool,
    /// Whether it holds more than `mod` declarations, attributes and comments.
    pub has_code: bool,
    /// Whether it is test code as a whole: its `mod` declaration, or one of an enclosing
    /// module, has a `cfg` that requires `test`.
    pub test: bool,
}

/// A path in `file`, at `line`, naming `path`: absolute, starting with a crate name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reference {
    pub file: PathBuf,
    pub line: usize,
    pub path: ModulePath,
    /// Whether the path lies in test code (see [`is_test`]).
    pub test: bool,
}

#[derive(Default)]
pub struct Code {
    /// Source files by path.
    pub files: BTreeMap<PathBuf, File>,
    /// Every module, file or inline, with the file that defines it.
    pub modules: BTreeMap<ModulePath, PathBuf>,
    /// The crate names other crates can use as the first segment of a path.
    pub libraries: BTreeSet<String>,
    /// Every path in the scanned files that names a module of the workspace, or an item in one.
    pub references: Vec<Reference>,
    /// What keeps the scan from being complete: unreadable or unparsable files, `mod`
    /// declarations without a file, `#[path]` attributes.
    pub problems: Vec<String>,
}

/// The module trees of `targets` and their references.
pub fn scan(targets: &[Target]) -> Code {
    let mut code = Code::default();
    let mut parsed = Vec::new();
    for target in targets {
        if target.library {
            code.libraries.insert(target.crate_name.clone());
        }
        let root = vec![target.crate_name.clone()];
        let file = Declared {
            mod_rs: true,
            crate_root: true,
            test: false,
        };
        code.add_file(&target.root, root, file, &mut parsed);
    }
    // Paths are resolved once every module is known: `use child::X` names a child module.
    for (path, syntax) in parsed {
        let file = &code.files[&path];
        let mut collector = Collector {
            code: &code,
            module: file.module.clone(),
            test: file.test,
            found: BTreeSet::new(),
        };
        collector.visit_file(&syntax);
        let found = collector.found;
        code.references
            .extend(found.into_iter().map(|(line, target, test)| Reference {
                file: path.clone(),
                line,
                path: target,
                test,
            }));
    }
    code
}

type Parsed = Vec<(PathBuf, syn::File)>;

/// How a module file was declared.
#[derive(Clone, Copy)]
struct Declared {
    /// `lib.rs`, `main.rs` or `mod.rs`, whose children sit next to it.
    mod_rs: bool,
    crate_root: bool,
    test: bool,
}

/// Whether `attrs` make an item test code: `#[test]` (also as `#[tokio::test]`), or a `cfg`
/// whose predicate cannot hold without `test`, such as `cfg(test)` or `cfg(all(test, unix))`.
/// `cfg(any(test, feature = "…"))` also compiles into production builds, so it is not.
pub fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        if path
            .segments
            .last()
            .is_some_and(|last| last.ident == "test")
        {
            return true;
        }
        path.is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|predicate| requires_test(&predicate))
    })
}

fn requires_test(predicate: &syn::Meta) -> bool {
    match predicate {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::NameValue(_) => false,
        syn::Meta::List(list) => {
            let Ok(nested) = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            if list.path.is_ident("all") {
                nested.iter().any(requires_test)
            } else if list.path.is_ident("any") {
                !nested.is_empty() && nested.iter().all(requires_test)
            } else {
                false
            }
        }
    }
}

impl Code {
    fn add_file(
        &mut self,
        path: &Path,
        module: ModulePath,
        declared: Declared,
        parsed: &mut Parsed,
    ) {
        let syntax = match fs::read_to_string(path)
            .map_err(|error| error.to_string())
            .and_then(|text| syn::parse_file(&text).map_err(|error| error.to_string()))
        {
            Ok(syntax) => syntax,
            Err(error) => {
                self.problems.push(format!("{}: {error}", path.display()));
                return;
            }
        };
        let parent = path.parent().unwrap_or(Path::new(""));
        // Children of `lib.rs`, `main.rs` and `mod.rs` sit next to them, those of `x.rs` in `x/`.
        let dir = match path.file_stem() {
            Some(stem) if !declared.mod_rs => parent.join(stem),
            _ => parent.to_path_buf(),
        };
        self.modules.insert(module.clone(), path.to_path_buf());
        self.add_modules(&syntax.items, path, &module, &dir, declared.test, parsed);
        let has_code = syntax
            .items
            .iter()
            .any(|item| !matches!(item, syn::Item::Mod(declared) if declared.content.is_none()));
        let file = File {
            module,
            crate_root: declared.crate_root,
            has_code,
            test: declared.test,
        };
        self.files.insert(path.to_path_buf(), file);
        parsed.push((path.to_path_buf(), syntax));
    }

    fn add_modules(
        &mut self,
        items: &[syn::Item],
        file: &Path,
        module: &[String],
        dir: &Path,
        test: bool,
        parsed: &mut Parsed,
    ) {
        for item in items {
            let syn::Item::Mod(declared) = item else {
                continue;
            };
            let test = test || is_test(&declared.attrs);
            let name = declared.ident.unraw().to_string();
            let child = [module, std::slice::from_ref(&name)].concat();
            if declared
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("path"))
            {
                self.problems.push(format!(
                    "{}: module {} uses #[path], which the check does not follow",
                    file.display(),
                    display(&child)
                ));
                continue;
            }
            match &declared.content {
                Some((_, items)) => {
                    self.modules.insert(child.clone(), file.to_path_buf());
                    self.add_modules(items, file, &child, &dir.join(&name), test, parsed);
                }
                None => {
                    let flat = dir.join(format!("{name}.rs"));
                    let nested = dir.join(&name).join("mod.rs");
                    let mut declared = Declared {
                        mod_rs: false,
                        crate_root: false,
                        test,
                    };
                    if flat.is_file() {
                        self.add_file(&flat, child, declared, parsed);
                    } else if nested.is_file() {
                        declared.mod_rs = true;
                        self.add_file(&nested, child, declared, parsed);
                    } else {
                        self.problems.push(format!(
                            "{}: no file for module {}",
                            file.display(),
                            display(&child)
                        ));
                    }
                }
            }
        }
    }

    /// The module of the workspace that `path` lies in: the longest known module that is a
    /// prefix of it.
    pub fn module_of<'a>(&self, path: &'a [String]) -> Option<&'a [String]> {
        (1..=path.len())
            .rev()
            .map(|len| &path[..len])
            .find(|prefix| self.modules.contains_key(*prefix))
    }
}

struct Collector<'a> {
    code: &'a Code,
    /// The module being visited, inline modules included.
    module: ModulePath,
    /// Whether the code being visited is test code.
    test: bool,
    found: BTreeSet<(usize, ModulePath, bool)>,
}

impl Collector<'_> {
    /// The absolute path that `segments` names from the current module, if it starts in the
    /// workspace: `crate`, `self`, `super`, a library of the workspace, or a child module.
    fn resolve(&self, segments: &[String]) -> Option<ModulePath> {
        let (first, rest) = segments.split_first()?;
        let mut path = match first.as_str() {
            "crate" => self.module[..1].to_vec(),
            "self" => self.module.clone(),
            "super" => self.module[..self.module.len().checked_sub(1)?].to_vec(),
            name if self.code.libraries.contains(name) => vec![name.to_owned()],
            name => {
                let child = [&self.module[..], &[name.to_owned()]].concat();
                if !self.code.modules.contains_key(&child) {
                    return None;
                }
                child
            }
        };
        let mut rest = rest.iter().peekable();
        if first == "super" {
            while rest.next_if(|segment| *segment == "super").is_some() {
                path.pop();
            }
        }
        path.extend(rest.cloned());
        (!path.is_empty()).then_some(path)
    }

    fn record(&mut self, line: usize, segments: &[String]) {
        if let Some(path) = self.resolve(segments) {
            self.found.insert((line, path, self.test));
        }
    }

    /// Visits code with `attrs` as test code if they make it so.
    fn scoped(&mut self, attrs: &[syn::Attribute], visit: impl FnOnce(&mut Self)) {
        let outer = self.test;
        self.test |= is_test(attrs);
        visit(self);
        self.test = outer;
    }

    fn use_tree(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.unraw().to_string());
                self.use_tree(&path.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(syn::UseName { ident })
            | syn::UseTree::Rename(syn::UseRename { ident, .. }) => {
                let line = ident.span().start().line;
                if ident == "self" {
                    self.record(line, prefix);
                } else {
                    let full = [&prefix[..], &[ident.unraw().to_string()]].concat();
                    self.record(line, &full);
                }
            }
            syn::UseTree::Glob(glob) => {
                self.record(glob.star_token.spans[0].start().line, prefix);
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.use_tree(tree, prefix);
                }
            }
        }
    }

    /// Paths inside macro invocations, which syn leaves as tokens: `first::second…`.
    fn tokens(&mut self, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();
        let mut index = 0;
        while index < trees.len() {
            match &trees[index] {
                TokenTree::Group(group) => self.tokens(group.stream()),
                TokenTree::Ident(first) => {
                    let mut segments = vec![first.unraw().to_string()];
                    while let (
                        Some(TokenTree::Punct(a)),
                        Some(TokenTree::Punct(b)),
                        Some(TokenTree::Ident(next)),
                    ) = (
                        trees.get(index + 1),
                        trees.get(index + 2),
                        trees.get(index + 3),
                    ) {
                        if a.as_char() != ':' || a.spacing() != Spacing::Joint || b.as_char() != ':'
                        {
                            break;
                        }
                        segments.push(next.unraw().to_string());
                        index += 3;
                    }
                    if segments.len() > 1 {
                        self.record(first.span().start().line, &segments);
                    }
                }
                _ => {}
            }
            index += 1;
        }
    }
}

/// The attributes that can make code test code, on the syntax that carries them most often.
fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
    match item {
        syn::ImplItem::Const(item) => &item.attrs,
        syn::ImplItem::Fn(item) => &item.attrs,
        syn::ImplItem::Type(item) => &item.attrs,
        syn::ImplItem::Macro(item) => &item.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
    match item {
        syn::TraitItem::Const(item) => &item.attrs,
        syn::TraitItem::Fn(item) => &item.attrs,
        syn::TraitItem::Type(item) => &item.attrs,
        syn::TraitItem::Macro(item) => &item.attrs,
        _ => &[],
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.scoped(item_attrs(item), |this| syn::visit::visit_item(this, item));
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        self.scoped(impl_item_attrs(item), |this| {
            syn::visit::visit_impl_item(this, item)
        });
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        self.scoped(trait_item_attrs(item), |this| {
            syn::visit::visit_trait_item(this, item)
        });
    }

    fn visit_field(&mut self, field: &'ast syn::Field) {
        self.scoped(&field.attrs, |this| syn::visit::visit_field(this, field));
    }

    fn visit_variant(&mut self, variant: &'ast syn::Variant) {
        self.scoped(&variant.attrs, |this| {
            syn::visit::visit_variant(this, variant)
        });
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        self.scoped(&arm.attrs, |this| syn::visit::visit_arm(this, arm));
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        self.scoped(&local.attrs, |this| syn::visit::visit_local(this, local));
    }

    fn visit_stmt_macro(&mut self, stmt: &'ast syn::StmtMacro) {
        self.scoped(&stmt.attrs, |this| syn::visit::visit_stmt_macro(this, stmt));
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        self.module.push(item.ident.unraw().to_string());
        syn::visit::visit_item_mod(self, item);
        self.module.pop();
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.use_tree(&item.tree, &mut Vec::new());
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        // A single segment names a value or type in scope, never a module.
        if path.segments.len() > 1 {
            let segments: Vec<String> = path
                .segments
                .iter()
                .map(|segment| segment.ident.unraw().to_string())
                .collect();
            let line = path.segments[0].ident.span().start().line;
            self.record(line, &segments);
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
        syn::visit::visit_macro(self, mac);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crate `demo` (library) with the given files, relative to `src/`.
    fn demo(files: &[(&str, &str)]) -> (tempfile::TempDir, Code) {
        let dir = tempfile::tempdir().expect("temporary directory");
        for (name, text) in files {
            let path = dir.path().join("src").join(name);
            fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
            fs::write(path, text).expect("write file");
        }
        let target = Target {
            crate_name: "demo".into(),
            root: dir.path().join("src/lib.rs"),
            library: true,
        };
        let other = Target {
            crate_name: "other".into(),
            root: dir.path().join("other.rs"),
            library: true,
        };
        fs::write(&other.root, "pub mod x {}").expect("write file");
        let code = scan(&[target, other]);
        (dir, code)
    }

    fn paths(code: &Code, file: &str) -> Vec<(usize, String)> {
        code.references
            .iter()
            .filter(|r| r.file.ends_with(file))
            .map(|r| (r.line, display(&r.path)))
            .collect()
    }

    fn module(path: &str) -> ModulePath {
        path.split("::").map(str::to_owned).collect()
    }

    #[test]
    fn follows_mod_declarations_into_flat_and_nested_files() {
        let (_dir, code) = demo(&[
            ("lib.rs", "pub mod a;\nmod b;\nmod c { pub mod d; }"),
            ("a.rs", "pub mod inner;"),
            ("a/inner.rs", "pub fn f() {}"),
            ("b/mod.rs", "pub fn g() {}"),
            ("c/d.rs", "pub fn h() {}"),
        ]);
        assert!(code.problems.is_empty(), "{:?}", code.problems);
        let modules: Vec<String> = code.modules.keys().map(|m| display(m)).collect();
        assert_eq!(
            modules,
            [
                "demo",
                "demo::a",
                "demo::a::inner",
                "demo::b",
                "demo::c",
                "demo::c::d",
                "other",
                "other::x"
            ]
        );
        let lib = &code.files[&code.modules[&module("demo")]];
        assert!(lib.crate_root);
        assert!(lib.has_code, "an inline module is code");
        assert!(!code.files[&code.modules[&module("demo::a")]].has_code);
    }

    #[test]
    fn reports_missing_module_files_and_path_attributes() {
        let (_dir, code) = demo(&[("lib.rs", "mod gone;\n#[path = \"x.rs\"]\nmod moved;")]);
        assert_eq!(code.problems.len(), 2, "{:?}", code.problems);
        assert!(code.problems[0].ends_with("no file for module demo::gone"));
        assert!(code.problems[1].contains("demo::moved uses #[path]"));
    }

    #[test]
    fn resolves_use_trees_against_the_current_module() {
        let (_dir, code) = demo(&[
            ("lib.rs", "pub mod a;\npub mod b;"),
            ("a.rs", "pub mod child;\npub use child::Thing;\nuse crate::b::{self, B as Renamed};\nuse super::b::*;\nuse other::x;\nuse std::fmt;"),
            ("a/child.rs", "pub struct Thing;\nuse super::super::b::B;"),
            ("b.rs", "pub struct B;"),
        ]);
        assert_eq!(
            paths(&code, "src/a.rs"),
            [
                (2, "demo::a::child::Thing".to_owned()),
                (3, "demo::b".to_owned()),
                (3, "demo::b::B".to_owned()),
                (4, "demo::b".to_owned()),
                (5, "other::x".to_owned()),
            ]
        );
        assert_eq!(
            paths(&code, "src/a/child.rs"),
            [(2, "demo::b::B".to_owned())]
        );
    }

    #[test]
    fn finds_paths_in_code_types_and_macros() {
        let (_dir, code) = demo(&[
            ("lib.rs", "pub mod a;\npub mod b;"),
            (
                "a.rs",
                "fn f(x: crate::b::B) -> other::x::Y {\n    let b = 1;\n    b.max(2);\n    \
                 format!(\"{}\", crate::b::name());\n    Vec::<super::b::B>::new();\n}\n\
                 #[cfg(test)]\nmod tests {\n    use super::super::b::B;\n}",
            ),
            ("b.rs", ""),
        ]);
        assert_eq!(
            paths(&code, "src/a.rs"),
            [
                (1, "demo::b::B".to_owned()),
                (1, "other::x::Y".to_owned()),
                (4, "demo::b::name".to_owned()),
                (5, "demo::b::B".to_owned()),
                (9, "demo::b::B".to_owned()),
            ]
        );
    }

    fn test_paths(code: &Code, file: &str) -> Vec<(usize, bool)> {
        code.references
            .iter()
            .filter(|r| r.file.ends_with(file))
            .map(|r| (r.line, r.test))
            .collect()
    }

    #[test]
    fn marks_references_in_test_code() {
        let (_dir, code) = demo(&[
            (
                "lib.rs",
                "pub mod a;\npub mod b;\n#[cfg(all(test, unix))]\nmod tests;\n#[cfg(not(test))]\nmod real;",
            ),
            (
                "a.rs",
                "fn f() { other::x::f() }\n\
                 #[cfg(test)]\nmod tests {\n    fn g() { other::x::g() }\n}\n\
                 #[test]\nfn h() { other::x::h() }\n\
                 #[tokio::test]\nasync fn i() { other::x::i() }\n\
                 #[cfg(any(test, feature = \"ui-test-support\"))]\nfn j() { other::x::j() }\n\
                 struct S {\n    #[cfg(test)]\n    field: other::x::S,\n}\n\
                 impl S {\n    #[cfg(test)]\n    fn k() { other::x::k() }\n    fn l() { other::x::l() }\n}",
            ),
            ("b.rs", "#[cfg(test)]\nmod nested;"),
            ("b/nested.rs", "fn f() { other::x::f() }"),
            ("tests.rs", "fn f() { other::x::f() }"),
            ("real.rs", "fn f() { other::x::f() }"),
        ]);
        assert!(code.problems.is_empty(), "{:?}", code.problems);
        assert_eq!(
            test_paths(&code, "src/a.rs"),
            [
                (1, false),
                (4, true),
                (7, true),
                (9, true),
                (11, false),
                (14, true),
                (18, true),
                (19, false),
            ]
        );
        assert_eq!(test_paths(&code, "src/b/nested.rs"), [(1, true)]);
        assert_eq!(test_paths(&code, "src/tests.rs"), [(1, true)]);
        assert_eq!(test_paths(&code, "src/real.rs"), [(1, false)]);
        assert!(code.files[&code.modules[&module("demo::tests")]].test);
        assert!(!code.files[&code.modules[&module("demo::b")]].test);
    }

    #[test]
    fn module_of_is_the_longest_known_prefix() {
        let (_dir, code) = demo(&[("lib.rs", "pub mod a;"), ("a.rs", "pub fn f() {}")]);
        assert_eq!(
            code.module_of(&module("demo::a::f")),
            Some(&module("demo::a")[..])
        );
        assert_eq!(
            code.module_of(&module("demo::run")),
            Some(&module("demo")[..])
        );
        assert_eq!(code.module_of(&module("elsewhere::x")), None);
    }
}
