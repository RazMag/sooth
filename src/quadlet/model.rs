use std::path::PathBuf;

/// The kind of quadlet unit, derived from its file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Container,
    Volume,
    Network,
    Pod,
    Kube,
    Build,
    Image,
}

impl UnitKind {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "container" => Some(Self::Container),
            "volume" => Some(Self::Volume),
            "network" => Some(Self::Network),
            "pod" => Some(Self::Pod),
            "kube" => Some(Self::Kube),
            "build" => Some(Self::Build),
            "image" => Some(Self::Image),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Volume => "volume",
            Self::Network => "network",
            Self::Pod => "pod",
            Self::Kube => "kube",
            Self::Build => "build",
            Self::Image => "image",
        }
    }

    /// The `[Section]` header quadlet requires as the file's primary section.
    pub fn primary_section(self) -> &'static str {
        match self {
            Self::Container => "Container",
            Self::Volume => "Volume",
            Self::Network => "Network",
            Self::Pod => "Pod",
            Self::Kube => "Kube",
            Self::Build => "Build",
            Self::Image => "Image",
        }
    }

    pub fn all() -> [Self; 7] {
        [
            Self::Container,
            Self::Volume,
            Self::Network,
            Self::Pod,
            Self::Kube,
            Self::Build,
            Self::Image,
        ]
    }
}

/// A single `[Section]` block, preserving key order and duplicate keys
/// exactly as they appeared in the source file (quadlet files legitimately
/// repeat keys like `Volume=` or `Environment=`).
#[derive(Debug, Clone, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// A quadlet unit file as discovered on disk, e.g. `myapp.container`.
#[derive(Debug, Clone)]
pub struct QuadletUnit {
    pub file_name: String,
    pub path: PathBuf,
    pub kind: UnitKind,
    pub sections: Vec<Section>,
    /// The exact original file contents. Edits are applied by patching this
    /// text rather than fully re-serializing `sections`, so comments and
    /// formatting the app didn't touch survive untouched.
    pub raw: String,
}

impl QuadletUnit {
    pub fn service_name(&self) -> String {
        super::naming::service_name(&self.file_name)
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// True for template units (`name@.container`) — managed read-only in v1
    /// since instantiation is meaningful extra UI surface for rare usage.
    pub fn is_template(&self) -> bool {
        super::naming::is_template(&self.file_name)
    }

    pub fn description(&self) -> Option<&str> {
        self.section(self.kind.primary_section())
            .and_then(|s| s.get("Image").or_else(|| s.get("Network")).or_else(|| s.get("VolumeName")))
    }
}
