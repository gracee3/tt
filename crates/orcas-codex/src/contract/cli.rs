use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, ExprLit, Fields, GenericArgument, Item, ItemEnum, ItemStruct, Lit, Meta,
    PathArguments, Type,
};

use super::ContractError;
use super::read_to_string;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliContract {
    pub entrypoint: CliCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliCommand {
    pub name: String,
    pub source_type: String,
    pub source_file: String,
    pub kind: CliCommandKind,
    pub docs: Vec<String>,
    pub attrs: Vec<String>,
    pub aliases: Vec<String>,
    pub hidden: bool,
    pub args: Vec<CliArg>,
    pub subcommands: Vec<CliCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliCommandKind {
    Parser,
    Args,
    SubcommandEnum,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliArg {
    pub name: String,
    pub ty: String,
    pub kind: CliArgKind,
    pub docs: Vec<String>,
    pub attrs: Vec<String>,
    pub required: bool,
    pub repeated: bool,
    pub global: bool,
    pub hidden: bool,
    pub flatten: Option<Box<CliCommand>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliArgKind {
    Positional,
    Flag,
    Flatten,
    Subcommand,
    Skip,
}

pub fn load_contract(root: impl AsRef<Path>) -> Result<CliContract, ContractError> {
    let root = root.as_ref();
    let files = [
        ("cli/src/main.rs", root.join("cli/src/main.rs")),
        ("cli/src/mcp_cmd.rs", root.join("cli/src/mcp_cmd.rs")),
        ("exec/src/cli.rs", root.join("exec/src/cli.rs")),
        ("tui/src/cli.rs", root.join("tui/src/cli.rs")),
        (
            "utils/cli/src/config_override.rs",
            root.join("utils/cli/src/config_override.rs"),
        ),
        (
            "utils/cli/src/approval_mode_cli_arg.rs",
            root.join("utils/cli/src/approval_mode_cli_arg.rs"),
        ),
        (
            "utils/cli/src/sandbox_mode_cli_arg.rs",
            root.join("utils/cli/src/sandbox_mode_cli_arg.rs"),
        ),
        (
            "app-server/src/transport/auth.rs",
            root.join("app-server/src/transport/auth.rs"),
        ),
    ];
    let index = SourceIndex::load(&files)?;
    let entrypoint = index.command_from_type("cli/src/main.rs", "MultitoolCli")?;
    Ok(CliContract {
        entrypoint: CliCommand {
            name: "codex".to_string(),
            ..entrypoint
        },
    })
}

impl CliContract {
    pub fn command_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        collect_command_paths(&self.entrypoint, "", &mut paths);
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn arg_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        collect_arg_paths(&self.entrypoint, "", &mut paths);
        paths.sort();
        paths.dedup();
        paths
    }
}

fn collect_command_paths(command: &CliCommand, prefix: &str, paths: &mut Vec<String>) {
    let path = if prefix.is_empty() {
        command.name.clone()
    } else {
        format!("{prefix} {}", command.name)
    };
    paths.push(path.clone());
    for subcommand in &command.subcommands {
        collect_command_paths(subcommand, &path, paths);
    }
}

fn collect_arg_paths(command: &CliCommand, prefix: &str, paths: &mut Vec<String>) {
    let path = if prefix.is_empty() {
        command.name.clone()
    } else {
        format!("{prefix} {}", command.name)
    };
    for arg in &command.args {
        let arg_path = format!("{path} {}", arg.name);
        paths.push(arg_path.clone());
        if let Some(flatten) = &arg.flatten {
            for nested_arg in &flatten.args {
                collect_flattened_arg_paths(&path, nested_arg, paths);
            }
            for nested_subcommand in &flatten.subcommands {
                collect_command_paths(nested_subcommand, &path, paths);
            }
        }
    }
    for subcommand in &command.subcommands {
        collect_arg_paths(subcommand, &path, paths);
    }
}

fn collect_flattened_arg_paths(prefix: &str, arg: &CliArg, paths: &mut Vec<String>) {
    let arg_path = format!("{prefix} {}", arg.name);
    paths.push(arg_path.clone());
    if let Some(flatten) = &arg.flatten {
        for nested_arg in &flatten.args {
            collect_flattened_arg_paths(prefix, nested_arg, paths);
        }
        for nested_subcommand in &flatten.subcommands {
            collect_command_paths(nested_subcommand, prefix, paths);
        }
    }
}

struct SourceIndex {
    files: BTreeMap<String, SourceFile>,
}

impl SourceIndex {
    fn load(paths: &[(&str, PathBuf)]) -> Result<Self, ContractError> {
        let mut files = BTreeMap::new();
        for (key, path) in paths {
            let path = path.as_path();
            let source = read_to_string(path)?;
            let syntax = syn::parse_file(&source).map_err(|source| ContractError::Syn {
                path: path.to_path_buf(),
                source,
            })?;
            files.insert((*key).to_string(), SourceFile::from_syntax(path, syntax));
        }
        Ok(Self { files })
    }

    fn command_from_type(
        &self,
        file_key: &str,
        type_name: &str,
    ) -> Result<CliCommand, ContractError> {
        let file = self
            .files
            .get(file_key)
            .ok_or_else(|| ContractError::Unsupported(format!("missing file {file_key}")))?;
        file.command_from_type(self, type_name)
    }
}

struct SourceFile {
    path: String,
    structs: BTreeMap<String, ItemStruct>,
    enums: BTreeMap<String, ItemEnum>,
}

impl SourceFile {
    fn from_syntax(path: &Path, syntax: syn::File) -> Self {
        let mut structs = BTreeMap::new();
        let mut enums = BTreeMap::new();
        for item in syntax.items {
            match item {
                Item::Struct(item) => {
                    structs.insert(item.ident.to_string(), item);
                }
                Item::Enum(item) => {
                    enums.insert(item.ident.to_string(), item);
                }
                _ => {}
            }
        }
        Self {
            path: path.to_string_lossy().to_string(),
            structs,
            enums,
        }
    }

    fn command_from_type(
        &self,
        index: &SourceIndex,
        type_name: &str,
    ) -> Result<CliCommand, ContractError> {
        if let Some(item) = self.structs.get(type_name) {
            return Ok(self.command_from_struct(index, item, type_name));
        }
        if let Some(item) = self.enums.get(type_name) {
            return Ok(self.command_from_enum(index, item, type_name));
        }
        Err(ContractError::Unsupported(format!(
            "type {type_name} not found in {}",
            self.path
        )))
    }

    fn command_from_struct(
        &self,
        index: &SourceIndex,
        item: &ItemStruct,
        type_name: &str,
    ) -> CliCommand {
        let attrs = clap_attrs(&item.attrs);
        let docs = doc_lines(&item.attrs);
        let kind = if has_derive(&item.attrs, "Args") {
            CliCommandKind::Args
        } else {
            CliCommandKind::Parser
        };
        let args = match &item.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .map(|field| self.arg_from_field(index, field))
                .collect(),
            Fields::Unnamed(fields) => fields
                .unnamed
                .iter()
                .enumerate()
                .map(|(index_field, field)| self.arg_from_unnamed_field(index, field, index_field))
                .collect(),
            Fields::Unit => Vec::new(),
        };

        let subcommands = match &item.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .find_map(|field| {
                    if has_attr(&field.attrs, "subcommand") {
                        cli_type_name(&field.ty)
                            .and_then(|nested| index.resolve_type(&nested))
                            .map(|command| command.subcommands)
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            Fields::Unnamed(fields) => fields
                .unnamed
                .iter()
                .find_map(|field| {
                    if has_attr(&field.attrs, "subcommand") {
                        cli_type_name(&field.ty)
                            .and_then(|nested| index.resolve_type(&nested))
                            .map(|command| command.subcommands)
                    } else {
                        None
                    }
                })
                .unwrap_or_default(),
            Fields::Unit => Vec::new(),
        };

        CliCommand {
            name: command_name(type_name, &item.attrs),
            source_type: type_name.to_string(),
            source_file: self.path.clone(),
            kind,
            docs,
            attrs,
            aliases: command_aliases(&item.attrs),
            hidden: has_attr(&item.attrs, "hide"),
            args,
            subcommands,
        }
    }

    fn command_from_enum(
        &self,
        index: &SourceIndex,
        item: &ItemEnum,
        type_name: &str,
    ) -> CliCommand {
        let attrs = clap_attrs(&item.attrs);
        let docs = doc_lines(&item.attrs);
        let args = Vec::new();
        let subcommands = item
            .variants
            .iter()
            .map(|variant| self.command_from_variant(index, variant))
            .collect();

        CliCommand {
            name: command_name(type_name, &item.attrs),
            source_type: type_name.to_string(),
            source_file: self.path.clone(),
            kind: CliCommandKind::SubcommandEnum,
            docs,
            attrs,
            aliases: command_aliases(&item.attrs),
            hidden: has_attr(&item.attrs, "hide"),
            args,
            subcommands,
        }
    }

    fn command_from_variant(&self, index: &SourceIndex, variant: &syn::Variant) -> CliCommand {
        let attrs = clap_attrs(&variant.attrs);
        let docs = doc_lines(&variant.attrs);
        let name = variant_command_name(variant);
        let (args, source_type, kind) = match &variant.fields {
            Fields::Unit => (Vec::new(), variant.ident.to_string(), CliCommandKind::Args),
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let inner = &fields.unnamed[0].ty;
                let inner_type = cli_type_name(inner).unwrap_or_else(|| variant.ident.to_string());
                let command = index
                    .resolve_type(&inner_type)
                    .unwrap_or_else(|| fallback_command_for_type(&inner_type));
                return CliCommand {
                    name,
                    source_type: inner_type,
                    source_file: command.source_file.clone(),
                    kind: command.kind.clone(),
                    docs,
                    attrs,
                    aliases: command_aliases(&variant.attrs),
                    hidden: has_attr(&variant.attrs, "hide"),
                    args: command.args,
                    subcommands: command.subcommands,
                };
            }
            Fields::Unnamed(fields) => {
                let args = fields
                    .unnamed
                    .iter()
                    .enumerate()
                    .map(|(index_field, field)| {
                        self.arg_from_unnamed_field(index, field, index_field)
                    })
                    .collect();
                (args, variant.ident.to_string(), CliCommandKind::Args)
            }
            Fields::Named(fields) => {
                let args = fields
                    .named
                    .iter()
                    .map(|field| self.arg_from_field(index, field))
                    .collect();
                (args, variant.ident.to_string(), CliCommandKind::Args)
            }
        };

        CliCommand {
            name,
            source_type,
            source_file: self.path.clone(),
            kind,
            docs,
            attrs,
            aliases: command_aliases(&variant.attrs),
            hidden: has_attr(&variant.attrs, "hide"),
            args,
            subcommands: Vec::new(),
        }
    }

    fn arg_from_field(&self, index: &SourceIndex, field: &syn::Field) -> CliArg {
        let attrs = clap_attrs(&field.attrs);
        let docs = doc_lines(&field.attrs);
        let name = field
            .ident
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "field".to_string());
        let ty = type_tokens(&field.ty);
        let flatten = if has_attr(&field.attrs, "flatten") {
            cli_type_name(&field.ty).and_then(|nested| {
                index
                    .resolve_type(&nested)
                    .map(Box::new)
                    .or_else(|| Some(Box::new(fallback_command_for_type(&nested))))
            })
        } else {
            None
        };
        let kind = if has_attr(&field.attrs, "subcommand") {
            CliArgKind::Subcommand
        } else if has_attr(&field.attrs, "flatten") {
            CliArgKind::Flatten
        } else if has_attr(&field.attrs, "skip") {
            CliArgKind::Skip
        } else if is_positional(field) {
            CliArgKind::Positional
        } else {
            CliArgKind::Flag
        };

        CliArg {
            name,
            ty,
            kind,
            docs,
            attrs,
            required: is_required(field),
            repeated: is_repeated(field),
            global: has_attr(&field.attrs, "global"),
            hidden: has_attr(&field.attrs, "hide"),
            flatten,
        }
    }

    fn arg_from_unnamed_field(
        &self,
        index: &SourceIndex,
        field: &syn::Field,
        index_field: usize,
    ) -> CliArg {
        let mut arg = self.arg_from_field(index, field);
        arg.name = format!("arg_{index_field}");
        arg
    }
}

impl SourceIndex {
    fn resolve_type(&self, type_name: &str) -> Option<CliCommand> {
        match type_name {
            "ExecCli" => return self.command_from_type("exec/src/cli.rs", "Cli").ok(),
            "TuiCli" => return self.command_from_type("tui/src/cli.rs", "Cli").ok(),
            "McpCli" => return self.command_from_type("cli/src/mcp_cmd.rs", "McpCli").ok(),
            "CliConfigOverrides" => {
                return self
                    .command_from_type("utils/cli/src/config_override.rs", "CliConfigOverrides")
                    .ok();
            }
            "ApprovalModeCliArg" => {
                return self
                    .command_from_type(
                        "utils/cli/src/approval_mode_cli_arg.rs",
                        "ApprovalModeCliArg",
                    )
                    .ok();
            }
            "SandboxModeCliArg" => {
                return self
                    .command_from_type("utils/cli/src/sandbox_mode_cli_arg.rs", "SandboxModeCliArg")
                    .ok();
            }
            "AppServerWebsocketAuthArgs" => {
                return self
                    .command_from_type(
                        "app-server/src/transport/auth.rs",
                        "AppServerWebsocketAuthArgs",
                    )
                    .ok();
            }
            _ => {}
        }
        self.files.values().find_map(|file| {
            if file.structs.contains_key(type_name) || file.enums.contains_key(type_name) {
                file.command_from_type(self, type_name).ok()
            } else {
                None
            }
        })
    }
}

fn fallback_command_for_type(type_name: &str) -> CliCommand {
    CliCommand {
        name: type_name.to_string(),
        source_type: type_name.to_string(),
        source_file: "<unknown>".to_string(),
        kind: CliCommandKind::Args,
        docs: Vec::new(),
        attrs: Vec::new(),
        aliases: Vec::new(),
        hidden: false,
        args: Vec::new(),
        subcommands: Vec::new(),
    }
}

fn cli_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => {
            let outer = path.path.segments.last()?.ident.to_string();
            match outer.as_str() {
                "Option" | "Vec" => segment_inner_type(path).and_then(cli_type_name),
                _ => Some(outer),
            }
        }
        _ => None,
    }
}

fn segment_inner_type(path: &syn::TypePath) -> Option<&Type> {
    path.path
        .segments
        .last()
        .and_then(|segment| match &segment.arguments {
            PathArguments::AngleBracketed(args) => args.args.iter().find_map(|arg| match arg {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            }),
            _ => None,
        })
}

fn type_tokens(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

fn outer_type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn is_option_type(ty: &Type) -> bool {
    matches!(outer_type_name(ty).as_deref(), Some("Option"))
}

fn is_vec_type(ty: &Type) -> bool {
    matches!(outer_type_name(ty).as_deref(), Some("Vec"))
}

fn is_required(field: &syn::Field) -> bool {
    if has_attr(&field.attrs, "skip")
        || has_attr(&field.attrs, "flatten")
        || has_attr(&field.attrs, "subcommand")
    {
        return false;
    }
    if has_value_like_attr(&field.attrs, "default_value")
        || has_value_like_attr(&field.attrs, "default_value_t")
    {
        return false;
    }
    !is_option_type(&field.ty) && !is_vec_type(&field.ty)
}

fn is_repeated(field: &syn::Field) -> bool {
    is_vec_type(&field.ty)
        || has_value_like_attr(&field.attrs, "action")
        || has_value_like_attr(&field.attrs, "num_args")
        || has_value_like_attr(&field.attrs, "value_delimiter")
}

fn is_positional(field: &syn::Field) -> bool {
    !has_any_named_attr(&field.attrs, &["long", "short", "visible_alias", "alias"])
        && !has_attr(&field.attrs, "subcommand")
        && !has_attr(&field.attrs, "flatten")
}

fn has_any_named_attr(attrs: &[Attribute], names: &[&str]) -> bool {
    attrs
        .iter()
        .map(|attr| attr.meta.to_token_stream().to_string())
        .any(|text| names.iter().any(|name| text.contains(name)))
}

fn has_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs
        .iter()
        .map(|attr| attr.meta.to_token_stream().to_string())
        .any(|text| text.contains(name))
}

fn has_derive(attrs: &[Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }
        attr.to_token_stream().to_string().contains(name)
    })
}

fn has_value_like_attr(attrs: &[Attribute], name: &str) -> bool {
    attrs
        .iter()
        .map(|attr| attr.meta.to_token_stream().to_string())
        .any(|text| text.contains(name))
}

fn command_name(type_name: &str, attrs: &[Attribute]) -> String {
    if let Some(name) = extract_named_value(attrs, "name") {
        return name;
    }
    kebab_case(type_name)
}

fn command_aliases(attrs: &[Attribute]) -> Vec<String> {
    extract_string_values(attrs, "visible_alias")
        .into_iter()
        .chain(extract_string_values(attrs, "alias"))
        .collect()
}

fn variant_command_name(variant: &syn::Variant) -> String {
    if let Some(name) = extract_named_value(&variant.attrs, "name") {
        return name;
    }
    kebab_case(&variant.ident.to_string())
}

fn extract_named_value(attrs: &[Attribute], key: &str) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("clap")
            && !attr.path().is_ident("arg")
            && !attr.path().is_ident("command")
        {
            return None;
        }
        let meta = attr.meta.clone();
        match meta {
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                .ok()
                .and_then(|items| {
                    items.into_iter().find_map(|item| match item {
                        Meta::NameValue(name_value) if name_value.path.is_ident(key) => {
                            expr_to_string(&name_value.value)
                        }
                        _ => None,
                    })
                }),
            Meta::NameValue(name_value) if name_value.path.is_ident(key) => {
                expr_to_string(&name_value.value)
            }
            _ => None,
        }
    })
}

fn extract_string_values(attrs: &[Attribute], key: &str) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| {
            attr.path().is_ident("clap")
                || attr.path().is_ident("arg")
                || attr.path().is_ident("command")
        })
        .flat_map(|attr| match attr.meta.clone() {
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
                .ok()
                .into_iter()
                .flat_map(|items| items.into_iter())
                .filter_map(|item| match item {
                    Meta::NameValue(name_value) if name_value.path.is_ident(key) => {
                        expr_to_string(&name_value.value)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>(),
            Meta::NameValue(name_value) if name_value.path.is_ident(key) => {
                expr_to_string(&name_value.value).into_iter().collect()
            }
            _ => Vec::new(),
        })
        .collect()
}

fn expr_to_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Some(value.value()),
        Expr::Lit(ExprLit {
            lit: Lit::Char(value),
            ..
        }) => Some(value.value().to_string()),
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| {
            if !attr.path().is_ident("doc") {
                return None;
            }
            match &attr.meta {
                Meta::NameValue(name_value) => expr_to_string(&name_value.value),
                _ => None,
            }
        })
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

fn clap_attrs(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|attr| {
            attr.path().is_ident("arg")
                || attr.path().is_ident("clap")
                || attr.path().is_ident("command")
        })
        .map(|attr| attr.meta.to_token_stream().to_string())
        .collect()
}

fn kebab_case(raw: &str) -> String {
    let mut out = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 && !out.ends_with('-') {
                out.push('-');
            }
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
        } else {
            out.push(ch);
        }
    }
    out
}
