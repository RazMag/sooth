use super::QuadletError;
use super::model::UnitKind;

/// Maps a quadlet file name to the systemd service unit name it generates,
/// per podman-systemd.unit(5): the quadlet extension is stripped and, for
/// `.volume`/`.network`/`.pod`/`.image`/`.build`, a matching infix
/// (`-volume`, ...) is inserted before `.service`; `.container` and `.kube`
/// generate a plain `<stem>.service`. Template files (`foo@.container`) and
/// instances (`foo@bar.container`) follow the same rule applied to the stem.
/// A file with no recognized quadlet extension falls back to `<stem>.service`.
pub fn service_name(file_name: &str) -> String {
    let stem = stem(file_name);
    let infix = kind_of(file_name)
        .map(UnitKind::service_infix)
        .unwrap_or("");
    format!("{stem}{infix}.service")
}

pub fn stem(file_name: &str) -> &str {
    file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
}

pub fn extension(file_name: &str) -> Option<&str> {
    file_name.rsplit_once('.').map(|(_, ext)| ext)
}

pub fn kind_of(file_name: &str) -> Option<UnitKind> {
    extension(file_name).and_then(UnitKind::from_extension)
}

/// A safe quadlet file-name stem (the part before the extension), as typed
/// into the "New" form's name field. Rejects anything that could escape the
/// quadlet directory or break the single-line `KEY=VALUE` sidecar format:
/// empty, over-long, a leading `.`, any `..`, path separators or control
/// characters. `@` is allowed so template/instance stems (`foo@`, `foo@bar`)
/// still work.
pub fn valid_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem.len() <= 200
        && !stem.starts_with('.')
        && !stem.contains("..")
        && !stem.contains(['/', '\\', '\0', '\n', '\r', ' '])
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'))
}

/// Builds a full quadlet file name from a user-typed stem and the kind its
/// "New" page implies, e.g. `("myapp", Container) -> "myapp.container"`. This
/// is the one place a create request's file name is assembled, so
/// `valid_stem` here is also what keeps `dir.join(file_name)` from being
/// pointed outside the quadlet directory.
pub fn compose_file_name(stem: &str, kind: UnitKind) -> Result<String, QuadletError> {
    if !valid_stem(stem) {
        return Err(QuadletError::Validation(format!(
            "invalid file name '{stem}': use letters, digits, '_', '-', '.', '@'; no '/' or '..'"
        )));
    }
    Ok(format!("{stem}.{}", kind.extension()))
}

/// True for a template unit definition, `name@.container`, as opposed to a
/// concrete instance, `name@instance.container`, or a plain unit, `name.container`.
pub fn is_template(file_name: &str) -> bool {
    stem(file_name).ends_with('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_units() {
        assert_eq!(service_name("myapp.container"), "myapp.service");
        assert_eq!(service_name("data.volume"), "data-volume.service");
        assert_eq!(service_name("mynet.network"), "mynet-network.service");
        assert_eq!(service_name("mypod.pod"), "mypod-pod.service");
        assert_eq!(service_name("app.kube"), "app.service");
        assert_eq!(service_name("img.build"), "img-build.service");
        assert_eq!(service_name("img.image"), "img-image.service");
    }

    #[test]
    fn template_and_instance_units() {
        assert_eq!(service_name("foo@.container"), "foo@.service");
        assert_eq!(service_name("foo@bar.container"), "foo@bar.service");
        assert!(is_template("foo@.container"));
        assert!(!is_template("foo@bar.container"));
        assert!(!is_template("foo.container"));
    }

    #[test]
    fn extension_and_kind() {
        assert_eq!(extension("myapp.container"), Some("container"));
        assert_eq!(kind_of("myapp.container"), Some(UnitKind::Container));
        assert_eq!(kind_of("myapp.txt"), None);
    }

    #[test]
    fn valid_stem_accepts_reasonable_names() {
        assert!(valid_stem("myapp"));
        assert!(valid_stem("my-app_1"));
        assert!(valid_stem("web.api"));
        assert!(valid_stem("foo@"));
        assert!(valid_stem("foo@bar"));
    }

    #[test]
    fn valid_stem_rejects_traversal_and_junk() {
        assert!(!valid_stem(""));
        assert!(!valid_stem("../x"));
        assert!(!valid_stem("a/b"));
        assert!(!valid_stem("a\\b"));
        assert!(!valid_stem(".hidden"));
        assert!(!valid_stem("a..b"));
        assert!(!valid_stem("foo bar"));
        assert!(!valid_stem("foo\tbar"));
        assert!(!valid_stem(&"x".repeat(300)));
    }

    #[test]
    fn compose_file_name_appends_the_kind_extension() {
        assert_eq!(
            compose_file_name("myapp", UnitKind::Container).unwrap(),
            "myapp.container"
        );
        assert_eq!(
            compose_file_name("img1", UnitKind::Build).unwrap(),
            "img1.build"
        );
        assert!(compose_file_name("../evil", UnitKind::Container).is_err());
    }
}
