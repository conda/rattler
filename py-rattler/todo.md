- add more fields and methods

- subclass
  - package_record(base) -> repo_data_record(subclasses package_record) -> prefix_record(subclasses repo_data_record)
    - either find a hacky way to init a super class from subclass from an existing obj?
    - use a different `Props` class which exposes the properties.
    - try to instead do this from the FFI lib?
    - go the embedded route? not the 
