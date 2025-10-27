use rattler_conda_types::{PackageName, PackageRecord};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::js_sys;

use crate::{
    noarch_type::JsNoArchType, package_name::JsPackageName, platform::JsPlatform,
    version_with_source::JsVersionWithSource,
};

/// A single record in the Conda repodata. A single record refers to a single
/// binary distribution of a package on a Conda channel.
///
/// @public
#[wasm_bindgen(js_name = "PackageRecord")]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
pub struct JsPackageRecord {
    inner: PackageRecord,
}

impl From<PackageRecord> for JsPackageRecord {
    fn from(value: PackageRecord) -> Self {
        JsPackageRecord { inner: value }
    }
}

impl From<JsPackageRecord> for PackageRecord {
    fn from(value: JsPackageRecord) -> Self {
        value.inner
    }
}

impl AsRef<PackageRecord> for JsPackageRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.inner
    }
}

impl AsMut<PackageRecord> for JsPackageRecord {
    fn as_mut(&mut self) -> &mut PackageRecord {
        &mut self.inner
    }
}

#[wasm_bindgen(typescript_custom_section)]
const PACKAGE_RECORD_D_TS: &'static str = include_str!("package_record.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "PackageRecordJson")]
    pub type JsPackageRecordJson;
}

#[wasm_bindgen(js_class = "PackageRecord")]
impl JsPackageRecord {
    /// Constructs a new instance from the json representation of a
    /// PackageRecord.
    #[wasm_bindgen(constructor)]
    pub fn new(json: JsPackageRecordJson) -> Result<JsPackageRecord, crate::error::JsError> {
        let package_record: PackageRecord = serde_wasm_bindgen::from_value(json.into())?;
        Ok(JsPackageRecord::from(package_record))
    }

    /// Convert this instance to the canonical json representation of a
    /// PackageRecord.
    #[wasm_bindgen(js_name = "toJson")]
    pub fn to_json(&self) -> Result<JsPackageRecordJson, crate::error::JsError> {
        Ok(serde_wasm_bindgen::to_value(&self.inner)?.into())
    }
}

macro_rules! impl_package_record {
    ($name:ident, $js_class:literal) => {
        #[wasm_bindgen::prelude::wasm_bindgen(js_class = $js_class)]
        impl JsPackageRecord {
            /// Optionally the architecture the package supports. This is almost
            /// always the second part of the `subdir` field. Except for `64` which
            /// maps to `x86_64` and `32` which maps to `x86`. This will be undefined if
            /// the package is `noarch`.
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn arch(&self) -> Option<crate::platform::JsArch> {
                AsRef::<PackageRecord>::as_ref(self)
                    .arch
                    .clone()
                    .map(Into::into)
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_arch(
                &mut self,
                arch: crate::platform::JsArchOption,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).arch =
                    serde_wasm_bindgen::from_value(arch.into())?;
                Ok(())
            }

            /// The build string of the package.
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn build(&self) -> String {
                AsRef::<PackageRecord>::as_ref(self).build.clone()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_build(&mut self, build: String) {
                AsMut::<PackageRecord>::as_mut(self).build = build;
            }

            /// The build number of the package.
            #[wasm_bindgen::prelude::wasm_bindgen(getter, js_name = "buildNumber")]
            pub fn build_number(&self) -> usize {
                AsRef::<PackageRecord>::as_ref(self).build_number as _
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "buildNumber")]
            pub fn set_build_number(&mut self, build_number: usize) {
                AsMut::<PackageRecord>::as_mut(self).build_number = build_number as _;
            }

            /// Additional constraints on packages. `constrains` are different from
            /// `depends` in that packages specified in `depends` must be installed
            /// next to this package, whereas packages specified in `constrains` are
            /// not required to be installed, but if they are installed they must follow
            /// these constraints.
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn constrains(&self) -> Vec<String> {
                AsRef::<PackageRecord>::as_ref(self).constrains.clone()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_constrains(&mut self, constraints: Vec<String>) {
                AsMut::<PackageRecord>::as_mut(self).constrains = constraints;
            }

            /// Specification of packages this package depends on
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn depends(&self) -> Vec<String> {
                AsRef::<PackageRecord>::as_ref(self).depends.clone()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_depends(&mut self, depends: Vec<String>) {
                AsMut::<PackageRecord>::as_mut(self).depends = depends;
            }

            /// Features are a deprecated way to specify different feature sets for the
            /// conda solver. This is not supported anymore and should not be used.
            /// Instead, `mutex` packages should be used to specify
            /// mutually exclusive features.
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined"
            )]
            pub fn features(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self).features.clone().into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_features(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                features: Option<String>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).features = features;
            }

            /// The specific license of the package
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined"
            )]
            pub fn license(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self).license.clone().into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_license(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                license: Option<String>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).license = license;
            }

            /// The license family
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined",
                js_name = "licenseFamily"
            )]
            pub fn license_family(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self)
                    .license_family
                    .clone()
                    .into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "licenseFamily")]
            pub fn set_license_family(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                license_family: Option<String>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).license_family = license_family;
            }

            /// The name of the package
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn name(&self) -> JsPackageName {
                let name = AsRef::<PackageRecord>::as_ref(self).name.as_source();
                JsValue::from(name).into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_name(&mut self, name: JsPackageName) -> Result<(), crate::error::JsError> {
                let name = serde_wasm_bindgen::from_value::<String>(name.into())?;
                AsMut::<PackageRecord>::as_mut(self).name = PackageName::try_from(name)?;
                Ok(())
            }

            /// Optionally a MD5 hash of the package archive encoded as a hex string
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined"
            )]
            pub fn md5(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self)
                    .md5
                    .as_ref()
                    .map(|hash| format!("{hash:x}"))
                    .into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_md5(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                md5: Option<String>,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).md5 = md5
                    .map(|hash| {
                        rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&hash)
                            .ok_or_else(|| crate::error::JsError::InvalidHexMd5(hash))
                    })
                    .transpose()?;
                Ok(())
            }

            /// Deprecated md5 hash
            /// @deprecated Use `md5` instead
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined",
                js_name = "legacyBz2Md5"
            )]
            pub fn legacy_bz2_md5(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self)
                    .legacy_bz2_md5
                    .as_ref()
                    .map(|hash| format!("{hash:x}"))
                    .into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "legacyBz2Md5")]
            pub fn set_legacy_bz2_md5(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                legacy_bz2_md5: Option<String>,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).legacy_bz2_md5 = legacy_bz2_md5
                    .map(|hash| {
                        rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&hash)
                            .ok_or_else(|| crate::error::JsError::InvalidHexMd5(hash))
                    })
                    .transpose()?;
                Ok(())
            }

            /// Deprecated size of the package archive.
            /// @deprecated Use `size` instead
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "number | undefined",
                js_name = "legacyBz2Size"
            )]
            pub fn legacy_bz2_size(&self) -> Option<usize> {
                AsRef::<PackageRecord>::as_ref(self)
                    .legacy_bz2_size
                    .map(|size| size as usize)
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "legacyBz2Size")]
            pub fn set_legacy_bz2_size(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "number | undefined")]
                legacy_bz2_size: Option<usize>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).legacy_bz2_size =
                    legacy_bz2_size.map(|size| size as _);
            }

            /// Optionally a Sha256 hash of the package archive encoded as a hex string
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined"
            )]
            pub fn sha256(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self)
                    .sha256
                    .as_ref()
                    .map(|hash| format!("{hash:x}"))
                    .into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_sha256(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                sha256: Option<String>,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).sha256 = sha256
                    .map(|hash| {
                        rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&hash)
                            .ok_or_else(|| crate::error::JsError::InvalidHexSha256(hash))
                    })
                    .transpose()?;
                Ok(())
            }

            /// Optionally the platform the package supports.
            /// Note that this does not match the `Platform` type, but is only the first
            /// part of the platform (e.g. `linux`, `osx`, `win`, ...).
            /// The `subdir` field contains the `Platform` enum.
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined"
            )]
            pub fn platform(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self).platform.clone().into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_platform(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                platform: Option<String>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).platform = platform;
            }

            /// Optionally a path within the environment of the site-packages directory.
            /// This field is only present for python interpreter packages.
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "string | undefined",
                js_name = "pythonSitePackagesPath"
            )]
            pub fn python_site_packages_path(&self) -> Option<String> {
                AsRef::<PackageRecord>::as_ref(self)
                    .python_site_packages_path
                    .clone()
                    .into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "pythonSitePackagesPath")]
            pub fn set_python_site_packages_path(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "string | undefined")]
                python_site_packages_path: Option<String>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).python_site_packages_path =
                    python_site_packages_path;
            }

            /// The size of the package archive in bytes
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "number | undefined"
            )]
            pub fn size(&self) -> Option<usize> {
                AsRef::<PackageRecord>::as_ref(self)
                    .size
                    .map(|size| size as usize)
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_size(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "number | undefined")]
                size: Option<usize>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).size = size.map(|size| size as _);
            }

            /// The subdirectory where the package can be found
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn subdir(&self) -> JsPlatform {
                let subdir = &AsRef::<PackageRecord>::as_ref(self).subdir;
                JsValue::from(subdir).into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_subdir(
                &mut self,
                platform: JsPlatform,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).subdir =
                    serde_wasm_bindgen::from_value(platform.into())?;
                Ok(())
            }

            /// If this package is independent of architecture this field specifies in
            /// what way. See [`NoArchType`] for more information.
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn noarch(&self) -> JsNoArchType {
                AsRef::<PackageRecord>::as_ref(self).noarch.clone().into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_noarch(
                &mut self,
                noarch: JsNoArchType,
            ) -> Result<(), crate::error::JsError> {
                AsMut::<PackageRecord>::as_mut(self).noarch = noarch.try_into()?;
                Ok(())
            }

            /// The date this entry was created.
            #[wasm_bindgen::prelude::wasm_bindgen(
                getter,
                unchecked_return_type = "Date | undefined"
            )]
            pub fn timestamp(&self) -> Option<js_sys::Date> {
                AsRef::<PackageRecord>::as_ref(self)
                    .timestamp
                    .map(|ts| ts.into_datetime().into())
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_timestamp(
                &mut self,
                #[wasm_bindgen::prelude::wasm_bindgen(unchecked_param_type = "Date | undefined")]
                timestamp: Option<js_sys::Date>,
            ) {
                AsMut::<PackageRecord>::as_mut(self).timestamp = timestamp.map(|date| {
                    let datetime: chrono::DateTime<chrono::Utc> = date.into();
                    datetime.into()
                });
            }

            /// Track features are nowadays only used to downweight packages (ie. give
            /// them less priority). To that effect, the package is downweighted
            /// by the number of track_features.
            #[wasm_bindgen::prelude::wasm_bindgen(getter, js_name = "trackFeatures")]
            pub fn track_features(&self) -> Vec<String> {
                AsRef::<PackageRecord>::as_ref(self).track_features.clone()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter, js_name = "trackFeatures")]
            pub fn set_track_features(&mut self, track_features: Vec<String>) {
                AsMut::<PackageRecord>::as_mut(self).track_features = track_features;
            }

            /// The version of the package
            #[wasm_bindgen::prelude::wasm_bindgen(getter)]
            pub fn version(&self) -> JsVersionWithSource {
                AsRef::<PackageRecord>::as_ref(self).version.clone().into()
            }

            #[wasm_bindgen::prelude::wasm_bindgen(setter)]
            pub fn set_version(&mut self, version: JsVersionWithSource) {
                AsMut::<PackageRecord>::as_mut(self).version = version.into();
            }
        }
    };
}

impl_package_record!(JsPackageRecord, "PackageRecord");
