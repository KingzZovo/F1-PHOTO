//! Authentication primitives.
//!
//! - `password`: argon2id hash + verify.
//! - `jwt`: HS256 access tokens, 24h TTL by default.
//! - `extractor`: axum extractors that resolve the current user, enforce
//!   admin-only access, and per-project permissions.

pub mod extractor;
pub mod jwt;
pub mod password;

pub use extractor::{
    CurrentUser, DeletePerm, ManagePerm, MemberPerms, Perm, ProjectAccess, RequireAdmin,
    RequireProjectPerm, UploadPerm, ViewPerm,
};
pub use jwt::{Claims, JwtCodec};
