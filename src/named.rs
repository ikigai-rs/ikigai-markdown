//! Named mappings, discovered in the config home and read through the kernel.
//!
//! A mapping is found by NAME at `urn:markdown:mapping:{name}`, which serves the file
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
//! **Read by request, never off the disk** (Hypothesis H, ledger #2384). The endpoint
//! opens no file. It sources `urn:markdown:file:mappings/{name}/mapping.ttl` through
//! the kernel ([`MappingHome::file`]), and the HOST decides what answers there: it
//! binds a file module at [`FILE_TEMPLATE`] jailed to [`MappingHome::root`], the
//! config home's `markdown` directory ([`MappingHome`] shows the wiring with `ikigai-fs`).
//!
//! So reading a named mapping takes the authority that reading the file takes, held by
//! the CALLER: the endpoint declares `urn:cap:fs:read:*` (some file read grant), and the
//! mount checks the path (with `ikigai-fs`, a `urn:cap:fs:read:` grant over the mappings
//! directory). A host that binds nothing there serves no named mapping at all.
//!
//! **Golden thread.** The representation inherits the file's: with a cacheable file
//! mount the mapping is cached under `urn:markdown:file:mappings/{name}/mapping.ttl`,
//! which a kernel write to that file cuts, and which a host's watcher over the mount
//! cuts for an edit made outside ikigai. The lift sources the mapping through the
//! kernel, so it inherits the same thread, and a cut re-lifts every document under the
//! mapping with no rebuild. A host that mounts the files uncached gets a mapping, and
//! lifts, that recompute on every read.
//!
//! **Fail loud.** A name with nothing behind it is a typed [`Error::NotFound`] naming
//! the exact IRI it asked for, and why: no such file, or no file mount at all. It never
//! falls back to another mapping, or to no mapping: a lift that silently lost its
//! interpretation would produce a structurally valid graph with none of the meaning
//! asked for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, Error, Invocation, Iri, ReprType, Representation, Result, Verb,
};

/// The IRI template a named mapping is served at.
pub const MAPPING_TEMPLATE: &str = "urn:markdown:mapping:{name}";
/// The named-mapping endpoint's description id.
pub const MAPPING_ID: &str = "markdown-mapping";
/// Where the named-mapping endpoint reads the config home's `markdown` directory: the
/// template a host binds a file module at, jailed to [`MappingHome::root`].
pub const FILE_TEMPLATE: &str = "urn:markdown:file:{path}";
/// The IRI prefix of [`FILE_TEMPLATE`]: a file under the config home's `markdown`
/// directory is this followed by its path relative to that directory.
pub const FILE_PREFIX: &str = "urn:markdown:file:";

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
/// What reading a mapping file takes under `ikigai-fs`: some read grant, judged by path
/// where the file is served.
const FS_READ: &str = "urn:cap:fs:read:*";

/// Where named mappings are looked up: a config home, held from construction.
///
/// `Option`, not a fallible constructor: a process with no config home is
/// under-configured, not broken. The absence becomes an error only when someone
/// resolves a named mapping, and then it says what is missing.
///
/// Every method is path or IRI algebra; none touches the filesystem. The endpoint reads a
/// mapping by request, from the file mount a host binds at [`FILE_TEMPLATE`] over
/// [`root`](Self::root). With `ikigai-fs`, cacheable so that lifts stay cached:
///
/// ```
/// use std::sync::Arc;
/// # let scratch = std::env::temp_dir().join(format!("ikigai-markdown-doc-{}", std::process::id()));
/// # let home_dir = scratch.join("ikigai");
/// let home = Arc::new(ikigai_markdown::MappingHome::new(Some(home_dir)));
/// # std::fs::create_dir_all(home.dir("notes").unwrap()).unwrap();
/// let space = ikigai_markdown::space_with(home.clone()).bind(
///     ikigai_core::UriTemplate::parse(ikigai_markdown::FILE_TEMPLATE).unwrap(),
///     ikigai_fs::FileEndpoint::new(home.root().unwrap()).cacheable(),
/// );
/// # let _ = space;
/// # std::fs::remove_dir_all(&scratch).unwrap();
/// ```
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

    /// The config home's `markdown` directory: the root a host jails the file mount at
    /// [`FILE_TEMPLATE`] to. `None` when there is no config home.
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    /// let home = ikigai_markdown::MappingHome::new(Some(PathBuf::from("/cfg/ikigai")));
    /// assert_eq!(home.root().unwrap(), Path::new("/cfg/ikigai/markdown"));
    /// assert!(ikigai_markdown::MappingHome::new(None).root().is_none());
    /// ```
    pub fn root(&self) -> Option<PathBuf> {
        self.home.as_deref().map(|home| home.join("markdown"))
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
        let root = self.root().ok_or_else(|| no_home(name))?;
        Ok(root.join("mappings").join(name))
    }

    /// The Turtle file the mapping called `name` lives in, on the host's disk.
    pub fn mapping_path(&self, name: &str) -> Result<PathBuf> {
        Ok(self.dir(name)?.join("mapping.ttl"))
    }

    /// The directory of the mapping's companion SPARQL queries.
    pub fn queries_dir(&self, name: &str) -> Result<PathBuf> {
        Ok(self.dir(name)?.join("queries"))
    }

    /// The IRI the mapping called `name` is read from: its file under [`FILE_PREFIX`].
    /// It is also the golden thread the served mapping hangs from, the name a write to
    /// the file, or a watcher over the mount, cuts.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// let home = ikigai_markdown::MappingHome::new(Some(PathBuf::from("/cfg/ikigai")));
    /// assert_eq!(
    ///     home.file("notes").unwrap(),
    ///     "urn:markdown:file:mappings/notes/mapping.ttl"
    /// );
    /// assert!(home.file("a/b").is_err());
    /// ```
    pub fn file(&self, name: &str) -> Result<String> {
        check_name(name)?;
        if self.home.is_none() {
            return Err(no_home(name));
        }
        Ok(format!("{FILE_PREFIX}mappings/{name}/mapping.ttl"))
    }
}

fn no_home(name: &str) -> Error {
    Error::NotFound(format!(
        "mapping `{name}`: this process has no config home (neither \
         XDG_CONFIG_HOME nor HOME is set), so there is nowhere to look for it"
    ))
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

/// The named-mapping endpoint: `urn:markdown:mapping:{name}`, read through the kernel
/// from [`MappingHome::file`]. Built by [`mapping_endpoint`].
pub struct MappingEndpoint {
    home: Arc<MappingHome>,
}

/// `urn:markdown:mapping:{name}` over the given home.
pub fn mapping_endpoint(home: Arc<MappingHome>) -> MappingEndpoint {
    MappingEndpoint { home }
}

#[async_trait]
impl Endpoint for MappingEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        let name = inv
            .bindings
            .get("name")
            .ok_or_else(|| Error::MissingArgument("name".to_string()))?;
        let file = self.home.file(name)?;
        let iri = Iri::parse(&file)
            .map_err(|e| Error::Endpoint(format!("mapping `{name}`: {file}: {e}")))?;
        match inv.source(&iri).await {
            Ok(repr) => {
                Ok(Representation::new(ReprType::new("text/turtle"), repr.bytes).cacheable())
            }
            // The miss is a fact about the FILE: hang this answer from the file's thread,
            // so installing the mapping (a write there, or a watcher's cut) un-misses it.
            Err(Error::NotFound(why)) => {
                inv.depends_on(file.as_str());
                Err(Error::NotFound(format!(
                    "no mapping named `{name}`: {why} (expected at {file})"
                )))
            }
            Err(Error::Unresolved(_)) => {
                inv.depends_on(file.as_str());
                Err(Error::NotFound(format!(
                    "no mapping named `{name}`: nothing serves {file} in this kernel; a \
                     host serves named mappings by binding a file module at {FILE_TEMPLATE} \
                     over the config home's markdown directory"
                )))
            }
            Err(e) => Err(e),
        }
    }

    fn describe(&self) -> Description {
        Description::new(MAPPING_ID)
            .title("Named markdown mapping")
            .summary(
                "A mapping found by name in the config home: \
                 <config home>/markdown/mappings/{name}/mapping.ttl, served as Turtle. Pass \
                 its IRI as urn:markdown:lift's `mapping`. Read through the kernel from \
                 urn:markdown:file:mappings/{name}/mapping.ttl, which the host binds over \
                 the config home, so the caller needs read authority there, and the answer \
                 is cached under that file's golden thread. A name with nothing behind it is \
                 not-found, naming the IRI asked for; there is no fallback.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            // The authority the conventional mount (`ikigai-fs`) checks on the file this
            // reads, declared so the manifold offers the name only to a caller who can read
            // it. The wildcard: some `urn:cap:fs:read:` grant; the mount judges the path.
            .requires(FS_READ)
            .input(
                ArgSpec::new("name")
                    .summary("the mapping's directory name under markdown/mappings/")
                    .class(XSD_STRING)
                    .binding(),
            )
            .output("text/turtle")
    }
}
