//! Scoped document-service options; all PDF interpretation stays in the facade.
use super::{Command, Flag, Options, Source, UsageError, number, text, usage};
use pagura::files::FileScope;
use pagura::images::ImageTransferMode;
use pagura::metadata::MetadataOwner;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ServiceCommand {
    Files(FileScope),
    ExtractFile {
        selection: FileSelection,
        output: PathBuf,
    },
    CheckFile(FileSelection),
    Metadata(MetadataOwner),
    ExtractMetadata {
        owner: MetadataOwner,
        output: PathBuf,
    },
    Collection,
    Images(MetadataOwner),
    ExtractImage {
        owner: MetadataOwner,
        thumbnail: bool,
        mode: ImageTransferMode,
        output: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileSelection {
    pub(crate) scope: FileScope,
    pub(crate) index: usize,
    pub(crate) key: String,
    pub(crate) related: Option<usize>,
}

impl FileSelection {
    pub(crate) fn payload(&self) -> pagura::files::FilePayload<'_> {
        match self.related {
            Some(index) => pagura::files::FilePayload::Related {
                key: self.key.as_bytes(),
                index,
            },
            None => pagura::files::FilePayload::Embedded(self.key.as_bytes()),
        }
    }
}

fn owner(options: &Options) -> Result<MetadataOwner, UsageError> {
    match (options.page, options.object) {
        (Some(_), Some(_)) => Err(usage("--page and --object are mutually exclusive")),
        (Some(page), None) => Ok(MetadataOwner::Page(page)),
        (None, Some(object)) => Ok(MetadataOwner::Object(object)),
        (None, None) => Ok(MetadataOwner::Document),
    }
}

fn scope(options: &Options) -> Result<FileScope, UsageError> {
    Ok(match owner(options)? {
        MetadataOwner::Document if options.has(Flag::Associated) => FileScope::Associated,
        MetadataOwner::Document => FileScope::Catalog,
        MetadataOwner::Page(page) if options.has(Flag::Associated) => {
            FileScope::PageAssociated(page)
        }
        MetadataOwner::Page(page) => FileScope::PageAnnotations(page),
        MetadataOwner::Object(id) => FileScope::ObjectAssociated(id),
        _ => return Err(usage("unsupported owner")),
    })
}

pub(super) fn command(
    name: &str,
    operands: &[OsString],
    options: &Options,
) -> Result<Option<Command>, UsageError> {
    let scoped = [Flag::Associated, Flag::Object, Flag::Related]
        .iter()
        .any(|flag| options.has(*flag));
    let metadata = matches!(name, "metadata" | "extract-metadata") && options.has(Flag::Page);
    let added = matches!(
        name,
        "collection" | "images" | "extract-image" | "extract-thumbnail"
    );
    if !added && !metadata && !scoped {
        return Ok(None);
    }
    let allowed = match name {
        "attachments" => vec![
            Flag::Strict,
            Flag::Password,
            Flag::Page,
            Flag::Object,
            Flag::Associated,
        ],
        "extract-attachment" | "check-attachment" => vec![
            Flag::Strict,
            Flag::Password,
            Flag::Page,
            Flag::Object,
            Flag::Associated,
            Flag::Related,
        ],
        "metadata" | "extract-metadata" | "images" => {
            vec![Flag::Strict, Flag::Password, Flag::Page, Flag::Object]
        }
        "extract-image" | "extract-thumbnail" => vec![
            Flag::Strict,
            Flag::Password,
            Flag::Page,
            Flag::Object,
            Flag::Stored,
        ],
        "collection" => vec![Flag::Strict, Flag::Password],
        _ => return Ok(None),
    };
    options.allow(name, &allowed)?;
    let Some((input, rest)) = operands.split_first() else {
        return Err(usage(format!("{name} requires INPUT")));
    };
    let source: Source = options.chosen_source(input);
    let selection = |index, key| -> Result<FileSelection, UsageError> {
        Ok(FileSelection {
            scope: scope(options)?,
            index: number(index, "INDEX")?,
            key: text(key, "KEY")?.to_owned(),
            related: options.related,
        })
    };
    let request = match (name, rest) {
        ("attachments", []) => ServiceCommand::Files(scope(options)?),
        ("check-attachment", [index, key]) => ServiceCommand::CheckFile(selection(index, key)?),
        ("extract-attachment", [index, key, output]) => ServiceCommand::ExtractFile {
            selection: selection(index, key)?,
            output: output.into(),
        },
        ("metadata", []) => ServiceCommand::Metadata(owner(options)?),
        ("extract-metadata", [output]) => ServiceCommand::ExtractMetadata {
            owner: owner(options)?,
            output: output.into(),
        },
        ("collection", []) => ServiceCommand::Collection,
        ("images", []) => ServiceCommand::Images(match owner(options)? {
            MetadataOwner::Document => MetadataOwner::Page(0),
            owner => owner,
        }),
        ("extract-image" | "extract-thumbnail", [output]) => {
            let owner = owner(options)?;
            if matches!(owner, MetadataOwner::Document)
                || (name == "extract-image" && !matches!(owner, MetadataOwner::Object(_)))
            {
                return Err(usage(
                    "extract-image requires --object; extract-thumbnail requires --page or --object",
                ));
            }
            ServiceCommand::ExtractImage {
                owner,
                thumbnail: name == "extract-thumbnail",
                mode: if options.has(Flag::Stored) {
                    ImageTransferMode::Stored
                } else {
                    ImageTransferMode::Encoded
                },
                output: output.into(),
            }
        }
        _ => return Err(usage(format!("wrong number of arguments for {name}"))),
    };
    Ok(Some(Command::Service { source, request }))
}
