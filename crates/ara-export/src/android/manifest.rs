//! Deterministic namespace-aware plugin permission merging.
use crate::{ExportPreset, preset::Configuration};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
const ANDROID: &str = "http://schemas.android.com/apk/res/android";

/// Merges plugin permissions deterministically. Conflicting attributes fail
/// instead of silently broadening or narrowing a plugin's permission declaration.
pub fn merge_manifest(preset: &ExportPreset, plugins: &[String]) -> Result<String> {
    let mut permissions = BTreeMap::<(String, String), BTreeMap<String, String>>::new();
    for xml in plugins {
        let doc =
            roxmltree::Document::parse(xml).context("ARA-EXP-AND-020: invalid plugin manifest")?;
        ensure!(
            doc.root_element().tag_name().name() == "manifest",
            "ARA-EXP-AND-020: expected manifest root"
        );
        for node in doc.root_element().children().filter(|n| n.is_element()) {
            ensure!(
                matches!(
                    node.tag_name().name(),
                    "uses-permission" | "uses-permission-sdk-23"
                ),
                "ARA-EXP-AND-021: plugin merger accepts permission declarations only"
            );
            let name = node
                .attribute((ANDROID, "name"))
                .context("ARA-EXP-AND-020: permission requires android:name")?;
            ensure!(!name.is_empty(), "ARA-EXP-AND-020: empty permission name");
            let mut attrs = BTreeMap::new();
            for attr in node.attributes() {
                ensure!(
                    attr.namespace() == Some(ANDROID)
                        && matches!(
                            attr.name(),
                            "name" | "maxSdkVersion" | "usesPermissionFlags"
                        ),
                    "ARA-EXP-AND-021: unsupported permission attribute"
                );
                attrs.insert(attr.name().into(), attr.value().into());
            }
            let key = (node.tag_name().name().into(), name.into());
            if let Some(old) = permissions.get(&key) {
                ensure!(
                    old == &attrs,
                    "ARA-EXP-AND-022: conflicting permission {name}"
                );
            }
            permissions.insert(key, attrs);
        }
    }
    if preset.config != Configuration::Release {
        permissions
            .entry((
                "uses-permission".into(),
                "android.permission.INTERNET".into(),
            ))
            .or_insert_with(|| {
                BTreeMap::from([("name".into(), "android.permission.INTERNET".into())])
            });
    }
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<manifest xmlns:android=\"{ANDROID}\" package=\"{}\" android:versionCode=\"{}\" android:versionName=\"1.0\">\n  <uses-sdk android:minSdkVersion=\"{}\" android:targetSdkVersion=\"{}\"/>\n",
        preset.package, preset.version_code, preset.min_api, preset.target_api
    );
    for ((tag, _), attrs) in permissions {
        xml.push_str(&format!("  <{tag}"));
        for (name, value) in attrs {
            xml.push_str(&format!(
                " android:{name}=\"{}\"",
                quick_xml::escape::escape(&value)
            ));
        }
        xml.push_str("/>\n");
    }
    xml.push_str(&format!("  <application android:debuggable=\"{}\" android:hasCode=\"false\" android:extractNativeLibs=\"false\" android:label=\"Ara\">\n    <activity android:name=\"android.app.NativeActivity\" android:exported=\"true\">\n      <meta-data android:name=\"android.app.lib_name\" android:value=\"ara_export_player\"/>\n      <intent-filter><action android:name=\"android.intent.action.MAIN\"/><category android:name=\"android.intent.category.LAUNCHER\"/></intent-filter>\n    </activity>\n  </application>\n</manifest>\n", preset.config != Configuration::Release));
    Ok(xml)
}
