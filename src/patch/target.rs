//! The app a bundle is for: [`Target`], what `<target>` says, and [`manifest_id`], where the
//! APK's own identity is read from its decoded manifest. Every part a target gives must match
//! the APK — a bundle for one app is not run on another, or on a version it was not written
//! for.

use apkscope_core::markup::Element;

use super::bundle::{no_children, only, req};

/// The app a bundle is for: what `<target>` says, and every part given must match. A bundle
/// for one app is not run on another, or on a version it was not written for.
#[derive(Debug)]
pub struct Target {
    pub package: String,
    pub version_name: Option<String>,
    pub min_version_code: Option<u32>,
    pub max_version_code: Option<u32>,
}

impl Target {
    pub(super) fn parse(e: &Element) -> Result<Self, String> {
        only(
            e,
            &["package", "versionName", "minVersionCode", "maxVersionCode"],
        )?;
        no_children(e)?;
        let code = |k: &str| {
            e.attr(k)
                .map(|v| {
                    v.parse::<u32>().map_err(|_| {
                        format!("line {}: <target> {} is not a number: {v}", e.line, k)
                    })
                })
                .transpose()
        };
        Ok(Target {
            package: req(e, "package")?,
            version_name: e.attr("versionName").map(str::to_string),
            min_version_code: code("minVersionCode")?,
            max_version_code: code("maxVersionCode")?,
        })
    }

    /// Whether `(package, versionName, versionCode)` is the app this target names. A part the
    /// target checks that the APK does not say is a mismatch, not a pass.
    pub fn matches(
        &self,
        package: Option<&str>,
        name: Option<&str>,
        code: Option<u32>,
    ) -> Result<(), String> {
        match package {
            Some(p) if p == self.package => {}
            Some(p) => {
                return Err(format!(
                    "the bundle is for {}, the APK is {p}",
                    self.package
                ));
            }
            None => {
                return Err(
                    "the APK has no package name to check the bundle's <target> against".into(),
                );
            }
        }
        if let Some(want) = &self.version_name {
            match name {
                Some(got) if got == want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionName {want:?}, the APK has {got:?}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionName to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        if let Some(want) = self.min_version_code {
            match code {
                Some(got) if got >= want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionCode at least {want}, the APK has {got}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionCode to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        if let Some(want) = self.max_version_code {
            match code {
                Some(got) if got <= want => {}
                Some(got) => {
                    return Err(format!(
                        "the bundle is for versionCode at most {want}, the APK has {got}"
                    ));
                }
                None => {
                    return Err(
                        "the APK has no android:versionCode to check the bundle's <target> against"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}

/// `(package, versionName, versionCode)` of a decoded manifest's text.
pub fn manifest_id(text: &str) -> (Option<String>, Option<String>, Option<u32>) {
    let Some(e) = text.split('<').find(|e| e.starts_with("manifest")) else {
        return (None, None, None);
    };
    let e = &e[..e.find('>').unwrap_or(e.len())];
    (
        crate::search::attr(e, "package"),
        crate::search::attr(e, "versionName"),
        crate::search::attr(e, "versionCode").and_then(|v| v.parse().ok()),
    )
}

#[cfg(test)]
#[path = "../../tests/apkscope/patch_target.rs"]
mod tests;
