use super::{Intent, Metadata};
use crate::{Error, Result};
use std::path::Path;
fn unsupported() -> Error {
    Error::Scope("native patches are qualified only on Windows".into())
}
pub(super) fn metadata(_: &Path, _: bool) -> Result<Metadata> {
    Err(unsupported())
}
pub(super) fn stage_attributes(_: &Path, _: &Metadata) -> Result<()> {
    Err(unsupported())
}
pub(super) fn secure_stage(_: &Path, _: &Metadata) -> Result<()> {
    Err(unsupported())
}
pub(super) fn replace(_: &Intent, _: impl Fn() -> Result<()>) -> Result<()> {
    Err(unsupported())
}
