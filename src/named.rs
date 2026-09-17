//! Named mappings, discovered in the config home.
//!
//! A mapping is found by NAME at `urn:markdown:mapping:{name}`, which serves
//!
//! ```text
//! <config home>/markdown/mappings/{name}/mapping.ttl
//! ```
//!
//! as `text/turtle`. The config home is `ikigai-core`'s (`$XDG_CONFIG_HOME/ikigai`,
//! else `~/.config/ikigai`), taken ONCE when the space is built — see
//! [`MappingHome`] — so a test or a host serving someone else's home hands one in.
//! A mapping's companion queries, if it has any, sit beside it in `queries/*.rq`;
//! [`MappingHome::queries_dir`] names that directory for a tool that runs them.
//!
//! The lift takes the name as a reference like any other:
//!
//! ```text
//! source urn:file:notes/0007.md | urn:markdown:lift mapping=urn:markdown:mapping:notes
//! ```
//!
//! **Golden thread.** The representation is `.cacheable()` and depends on the thread
//! `urn:file:{absolute path of mapping.ttl}` — the name a filesystem watcher cuts. The
//! lift sources the mapping through the kernel, so it inherits that thread, and a cut
//! re-lifts every document under the mapping with no rebuild.
//!
//! **Fail loud.** A name with no `mapping.ttl` behind it is a typed
//! [`Error::NotFound`] naming the exact path it looked at. It never falls back to
//! another mapping, or to no mapping: a lift that silently lost its interpretation
//! would produce a structurally valid graph with none of the meaning asked for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ikigai_core::{
    ArgSpec, Description, Error, FnEndpoint, Invocation, ReprType, Representation, Result, Verb,
};

/// The IRI template a named mapping is served at.
pub const MAPPING_TEMPLATE: &str = "urn:markdown:mapping:{name}";
/// The named-mapping endpoint's description id.
pub const MAPPING_ID: &str = "markdown-mapping";

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// Where named mappings are looked up: a config home, held from construction.
///
/// `Option`, not a fallible constructor: a process with no config home is
/// under-configured, not broken. The absence becomes an error only when someone
/// resolves a named mapping, and then it says what is missing.
#[derive(Debug, Clone)]
pub struct MappingHome {
    home: Option<PathBuf>,
}

impl MappingHome {
    /// Named mappings under a stated config home (the `ikigai` directory itself, e.g.
    /// `~/.config/ikigai`).
    pub fn new(home: Option<PathBuf>) -> MappingHome {
        MappingHome { home }
    }

    /// Named mappings under **this machine's** config home: sugar over [`new`](Self::new)
    /// that reads the environment once, here, and never again.
    pub fn ambient() -> MappingHome {
        MappingHome::new(ikigai_core::config::config_home())
    }

    /// The config home, if this process has one.
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }

    /// The directory holding the mapping called `name`.
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    /// let home = ikigai_markdown::MappingHome::new(Some(PathBuf::from("/cfg/ikigai")));
    /// assert_eq!(
    ///     home.dir("notes").unwrap(),
    ///     Path::new("/cfg/ikigai/markdown/mappings/notes")
    /// );
    /// assert_eq!(
    ///     home.mapping_path("notes").unwrap(),
    ///     Path::new("/cfg/ikigai/markdown/mappings/notes/mapping.ttl")
    /// );
    /// assert_eq!(
    ///     home.queries_dir("notes").unwrap(),
    ///     Path::new("/cfg/ikigai/markdown/mappings/notes/queries")
    /// );
    /// // A name is one path segment: nothing that could leave the mappings directory.
    /// assert!(home.dir("../elsewhere").is_err());
    /// ```
    pub fn dir(&self, name: &str) -> Result<PathBuf> {
        check_name(name)?;
        let home = self.home.as_deref().ok_or_else(|| {
            Error::NotFound(format!(
                "mapping `{name}`: this process has no config home (neither \
                 XDG_CONFIG_HOME nor HOME is set), so there is nowhere to look for it"
            ))
        })?;
        Ok(home.join("markdown").join("mappings").join(name))
    }

    /// The Turtle file the mapping called `name` is read from.
    pub fn mapping_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.dir(name)?.join("mapping.ttl"))
    }

    /// The directory of the mapping's companion SPARQL queries.
    pub fn queries_dir(&self, name: &str) -> Result<PathBuf> {
        Ok(self.dir(name)?.join("queries"))
    }

    /// The golden thread a served mapping depends on: `urn:file:` and the absolute
    /// path, which is the name a filesystem watcher cuts.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// let home = ikigai_markdown::MappingHome::new(Some(PathBuf::from("/cfg/ikigai")));
    /// assert_eq!(
    ///     home.thread("notes").unwrap(),
    ///     "urn:file:/cfg/ikigai/markdown/mappings/notes/mapping.ttl"
    /// );
    /// ```
    pub fn thread(&self, name: &str) -> Result<String> {
        Ok(format!("urn:file:{}", self.mapping_path(name)?.display()))
    }

    /// Read the mapping called `name`, refusing loudly when it is not there.
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = self.mapping_path(name)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(format!(
                "no mapping named `{name}`: {} does not exist",
                path.display()
            ))),
            Err(e) => Err(Error::Endpoint(format!(
                "mapping `{name}`: cannot read {}: {e}",
                path.display()
            ))),
        }
    }
}

/// A name is a single, ordinary path segment.
fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument {
            name: "name".to_string(),
            detail: format!(
                "`{name}` is not a mapping name: use letters, digits, `-`, `_` and `.` \
                 (one directory name under markdown/mappings/)"
            ),
        })
    }
}

/// `urn:markdown:mapping:{name}` over the given home.
pub fn mapping_endpoint(home: Arc<MappingHome>) -> FnEndpoint {
    FnEndpoint::new(MAPPING_ID, move |inv: &Invocation<'_>| {
        let name = inv
            .bindings
            .get("name")
            .ok_or_else(|| Error::MissingArgument("name".to_string()))?;
        let bytes = home.read(name)?;
        Ok(Representation::new(ReprType::new("text/turtle"), bytes)
            .cacheable()
            .depends_on(home.thread(name)?))
    })
    .with_description(
        Description::new(MAPPING_ID)
            .title("Named markdown mapping")
            .summary(
                "A mapping found by name in the config home: \
                 <config home>/markdown/mappings/{name}/mapping.ttl, served as Turtle. Pass \
                 its IRI as urn:markdown:lift's `mapping`. Cached under the file's golden \
                 thread, so editing the file re-lifts. A name with no file behind it is \
                 not-found, naming the path looked at; there is no fallback.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("name")
                    .summary("the mapping's directory name under markdown/mappings/")
                    .class(XSD_STRING)
                    .binding(),
            )
            .output("text/turtle"),
    )
}
