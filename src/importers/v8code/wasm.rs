use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use wasmparser::{ExternalKind, KnownCustom, Name, Parser, Payload, TypeRef};

#[derive(Debug)]
pub(super) struct Function {
    pub index: u64,
    pub body_start: u64,
    pub body: Vec<u8>,
    pub operations: BTreeMap<u64, (u64, String)>,
    pub label: String,
}
pub(super) fn functions(bytes: &[u8]) -> Result<Vec<Function>> {
    let mut imports = 0u64;
    let mut output = vec![];
    let mut names = BTreeMap::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload? {
            Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    if matches!(import?.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                        imports += 1;
                    }
                }
            }
            Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export?;
                    if export.kind == ExternalKind::Func {
                        names.insert(u64::from(export.index), export.name.to_owned());
                    }
                }
            }
            Payload::CustomSection(reader) => {
                if let KnownCustom::Name(reader) = reader.as_known() {
                    for name in reader {
                        if let Name::Function(map) = name? {
                            for name in map {
                                let name = name?;
                                names.insert(u64::from(name.index), name.name.to_owned());
                            }
                        }
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                ensure!(
                    output.len() < 100_000,
                    "Wasm function count exceeds code custody limit"
                );
                let start = body.range().start;
                let mut operators = body.get_operators_reader()?;
                let mut operations = BTreeMap::new();
                while !operators.eof() {
                    let offset = operators.original_position() - start;
                    let operator = operators.read()?;
                    let debug = format!("{operator:?}");
                    let name = debug.split([' ', '{']).next().unwrap_or("").to_owned();
                    operations.insert(offset, (operators.original_position() - start, name));
                }
                output.push(Function {
                    index: imports + output.len() as u64,
                    body_start: start,
                    body: body.as_bytes().to_vec(),
                    operations,
                    label: String::new(),
                });
            }
            _ => {}
        }
    }
    for function in &mut output {
        function.label = names
            .remove(&function.index)
            .unwrap_or_else(|| format!("wasm-function[{}]", function.index));
    }
    Ok(output)
}
