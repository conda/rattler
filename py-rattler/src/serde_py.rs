//! Conversion between `serde` types and Python objects.
//!
//! Values are converted through [`serde_json::Value`], so the resulting Python
//! objects are built from `None`, `bool`, `int`, `float`, `str`, `list` and
//! `dict` only. This mirrors how these values are represented on disk (JSON /
//! YAML) and is all the bindings need.

use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyMapping, PySequence, PyString, PyTuple},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Number, Value};

/// Serialize a `serde` value into a Python object.
pub fn to_py<'py, T: Serialize + ?Sized>(
    py: Python<'py>,
    value: &T,
) -> PyResult<Bound<'py, PyAny>> {
    let value =
        serde_json::to_value(value).map_err(|err| PyValueError::new_err(err.to_string()))?;
    json_to_py(py, &value)
}

/// Deserialize a Python object into a `serde` value.
///
/// Raises `TypeError` when the object contains values that have no JSON
/// equivalent and `ValueError` when the data does not match the shape of `T`.
pub fn from_py<T: DeserializeOwned>(obj: &Bound<'_, PyAny>) -> PyResult<T> {
    let value = py_to_json(obj)?;
    serde_json::from_value(value).map_err(|err| PyValueError::new_err(err.to_string()))
}

fn json_to_py<'py>(py: Python<'py>, value: &Value) -> PyResult<Bound<'py, PyAny>> {
    Ok(match value {
        Value::Null => py.None().into_bound(py),
        Value::Bool(b) => PyBool::new(py, *b).to_owned().into_any(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any()
            } else {
                // Every non-integer `Number` is representable as an `f64`.
                n.as_f64().unwrap_or(f64::NAN).into_pyobject(py)?.into_any()
            }
        }
        Value::String(s) => PyString::new(py, s).into_any(),
        Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_py(py, item)?)?;
            }
            list.into_any()
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, json_to_py(py, item)?)?;
            }
            dict.into_any()
        }
    })
}

fn py_to_json(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // `bool` is a subclass of `int`, so it has to be checked first.
    if let Ok(b) = obj.cast::<PyBool>() {
        return Ok(Value::Bool(b.is_true()));
    }
    if obj.is_instance_of::<PyInt>() {
        if let Ok(i) = obj.extract::<i64>() {
            return Ok(Value::from(i));
        }
        if let Ok(u) = obj.extract::<u64>() {
            return Ok(Value::from(u));
        }
        return Err(PyValueError::new_err(format!(
            "integer {obj} is out of range"
        )));
    }
    if let Ok(f) = obj.cast::<PyFloat>() {
        return Number::from_f64(f.value())
            .map(Value::Number)
            .ok_or_else(|| PyValueError::new_err(format!("cannot represent float {obj}")));
    }
    if let Ok(s) = obj.cast::<PyString>() {
        return Ok(Value::String(s.to_str()?.to_owned()));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        return mapping_to_json(dict.iter());
    }
    if let Ok(list) = obj.cast::<PyList>() {
        return list.iter().map(|item| py_to_json(&item)).collect();
    }
    if let Ok(tuple) = obj.cast::<PyTuple>() {
        return tuple.iter().map(|item| py_to_json(&item)).collect();
    }
    if let Ok(mapping) = obj.cast::<PyMapping>() {
        let items = mapping.items()?;
        let mut pairs = Vec::with_capacity(items.len());
        for item in items.iter() {
            pairs.push(item.extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?);
        }
        return mapping_to_json(pairs.into_iter());
    }
    if let Ok(sequence) = obj.cast::<PySequence>() {
        let len = sequence.len()?;
        let mut items = Vec::with_capacity(len);
        for idx in 0..len {
            items.push(py_to_json(&sequence.get_item(idx)?)?);
        }
        return Ok(Value::Array(items));
    }
    Err(PyTypeError::new_err(format!(
        "unsupported type '{}'",
        obj.get_type().name()?
    )))
}

fn mapping_to_json<'py>(
    items: impl Iterator<Item = (Bound<'py, PyAny>, Bound<'py, PyAny>)>,
) -> PyResult<Value> {
    let mut map = Map::new();
    for (key, value) in items {
        let Ok(key) = key.cast::<PyString>() else {
            return Err(PyTypeError::new_err(format!(
                "dictionary keys must be strings, got '{}'",
                key.get_type().name()?
            )));
        };
        let key = key.to_str()?.to_owned();
        map.insert(key, py_to_json(&value)?);
    }
    Ok(Value::Object(map))
}
