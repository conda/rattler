mod conda_package_data;
pub(crate) mod deserialize;
pub(in crate::parse::models) mod pypi_package_data;
pub(in crate::parse::models) mod source_data;

pub(crate) use conda_package_data::CondaPackageDataModel;
pub(crate) use pypi_package_data::PypiPackageDataModel;
