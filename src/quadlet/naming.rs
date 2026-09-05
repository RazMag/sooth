use super::model::UnitKind;

/// Maps a quadlet file name to the systemd service unit name it generates,
/// per podman-systemd.unit(5): the quadlet extension is stripped and
/// `.service` is appended. Template files (`foo@.container`) and instances
/// (`foo@bar.container`) follow the same rule applied to the stem, e.g.
/// `foo@.container` -> `foo@.service`, `foo@bar.container` -> `foo@bar.service`.
pub fn service_name(file_name: &str) -> String {
    let stem = stem(file_name);
    format!("{stem}.service")
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
        assert_eq!(service_name("data.volume"), "data.service");
        assert_eq!(service_name("mynet.network"), "mynet.service");
        assert_eq!(service_name("mypod.pod"), "mypod.service");
        assert_eq!(service_name("app.kube"), "app.service");
        assert_eq!(service_name("img.build"), "img.service");
        assert_eq!(service_name("img.image"), "img.service");
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
}
