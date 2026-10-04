use std::str::FromStr;

use rattler_conda_types::{
    Flag, PackageName, PackageRecord, RepoDataRecord, package::DistArchiveIdentifier,
};
use serde::Serialize;
use url::Url;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::js_sys;

use crate::{
    JsError, JsResult, noarch_type::JsNoArchType, package_name::JsPackageName,
    package_record::impl_package_record, platform::JsPlatform,
    version_with_source::JsVersionWithSource,
};

/// A `PackageRecord` together with the channel it was fetched from and the
/// url and file name of its archive. The gateway returns records in this
/// form.
///
/// @public
#[wasm_bindgen(js_name = "RepoDataRecord")]
#[repr(transparent)]
#[derive(Eq, PartialEq)]
pub struct JsRepoDataRecord {
    inner: RepoDataRecord,
}

impl From<RepoDataRecord> for JsRepoDataRecord {
    fn from(value: RepoDataRecord) -> Self {
        JsRepoDataRecord { inner: value }
    }
}

impl From<JsRepoDataRecord> for RepoDataRecord {
    fn from(value: JsRepoDataRecord) -> Self {
        value.inner
    }
}

impl AsRef<RepoDataRecord> for JsRepoDataRecord {
    fn as_ref(&self) -> &RepoDataRecord {
        &self.inner
    }
}

impl AsRef<PackageRecord> for JsRepoDataRecord {
    fn as_ref(&self) -> &PackageRecord {
        &self.inner.package_record
    }
}

impl AsMut<PackageRecord> for JsRepoDataRecord {
    fn as_mut(&mut self) -> &mut PackageRecord {
        &mut self.inner.package_record
    }
}

#[wasm_bindgen(typescript_custom_section)]
const REPO_DATA_RECORD_D_TS: &'static str = include_str!("repo_data_record.d.ts");

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "RepoDataRecordJson")]
    pub type JsRepoDataRecordJson;
}

#[wasm_bindgen(js_class = "RepoDataRecord")]
impl JsRepoDataRecord {
    /// Constructs a new instance from the json representation of a
    /// RepoDataRecord, e.g. a record returned by `Gateway.query`.
    #[wasm_bindgen(constructor)]
    pub fn new(json: JsRepoDataRecordJson) -> JsResult<JsRepoDataRecord> {
        let record: RepoDataRecord = serde_wasm_bindgen::from_value(json.into())?;
        Ok(record.into())
    }

    /// Convert this instance to the canonical json representation of a
    /// RepoDataRecord.
    #[wasm_bindgen(js_name = "toJson")]
    pub fn to_json(&self) -> JsResult<JsRepoDataRecordJson> {
        let serializer = serde_wasm_bindgen::Serializer::json_compatible();
        Ok(self.inner.serialize(&serializer)?.into())
    }

    /// Compares this record with another record, e.g. to sort records.
    ///
    /// Records are ordered like their `PackageRecord`s: by name, then records
    /// with track features before records without, then by version, build
    /// number and timestamp. Returns `-1` if this record should be ordered
    /// before `other`, `0` if they are ordered the same and `1` if this
    /// record should be ordered after `other`.
    pub fn compare(
        &self,
        #[wasm_bindgen(param_description = "The record to compare with")] other: &Self,
    ) -> i8 {
        crate::utils::ordering_to_i8(self.inner.cmp(&other.inner))
    }

    /// The file name of the package archive, e.g.
    /// `python-3.12.0-h1234_0.conda`.
    #[wasm_bindgen(getter, js_name = "fileName")]
    pub fn file_name(&self) -> String {
        self.inner.identifier.to_file_name()
    }

    #[wasm_bindgen(setter, js_name = "fileName")]
    pub fn set_file_name(&mut self, file_name: String) -> JsResult<()> {
        self.inner.identifier = DistArchiveIdentifier::from_str(&file_name)
            .map_err(|_| JsError::InvalidFileName(file_name))?;
        Ok(())
    }

    /// The canonical url of the package archive.
    #[wasm_bindgen(getter)]
    pub fn url(&self) -> String {
        self.inner.url.to_string()
    }

    #[wasm_bindgen(setter)]
    pub fn set_url(&mut self, url: String) -> JsResult<()> {
        self.inner.url = Url::parse(&url).map_err(|_| JsError::InvalidUrl(url))?;
        Ok(())
    }

    /// The canonical url of the channel the package was fetched from, if
    /// known.
    #[wasm_bindgen(getter)]
    pub fn channel(&self) -> Option<String> {
        self.inner.channel.clone()
    }

    #[wasm_bindgen(setter)]
    pub fn set_channel(
        &mut self,
        #[wasm_bindgen(unchecked_param_type = "string | undefined")] channel: Option<String>,
    ) {
        self.inner.channel = channel;
    }
}

impl_package_record!(JsRepoDataRecord, "RepoDataRecord");
