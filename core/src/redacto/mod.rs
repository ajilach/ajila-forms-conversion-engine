//! Redacto output: PostgreSQL dump generation for the Ajila Redacto platform.
//!
//! Redacto stores a document as rows in the `app_redacto` schema rather than
//! as an AEM content package: a `documents` row carrying a language-neutral
//! *document configuration* JSON, one `document_version` per language, and a
//! set of reusable HTML text assets (`assets` + one `asset_version` per
//! language) that the configuration composes into the page.
//!
//! This module converts a [`StructuredNode`](crate::structured::StructuredNode)
//! tree into a [`RedactoDump`] (the typed intermediate representation, one
//! `Vec` per table) and serialises that dump to SQL. The split mirrors the XSD
//! target ([`generate_xsd_schema`](crate::xsd::generate_xsd_schema) →
//! [`XsdSchema::to_xml`](crate::xsd::XsdSchema::to_xml)) and lets tests assert
//! on the IR rather than on generated text.
//!
//! The target is intended for documents *without* input fields: fields are
//! skipped and reported in [`RedactoDump::warnings`].

mod content;
mod converter;
mod profile;
mod sql;

pub use content::render_block_html;
pub use converter::{generate_redacto_dump, generate_redacto_sql};
pub use profile::{RedactoConfig, RedactoProfile};
pub use sql::sql_string;

use serde::{Deserialize, Serialize};

// ============================================================================
// Enum column domains
// ============================================================================

/// `assets.asset_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetType {
    /// A rich-text asset (`TEXT`).
    Text,
    /// A binary image asset (`IMAGE`).
    Image,
}

/// `asset_version.status` / `document_version.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Editable draft.
    Draft,
    /// Submitted for review, still editable.
    InReview,
    /// Published, immutable.
    Released,
    /// Superseded.
    Deprecated,
}

/// `ownerships.owner_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerType {
    /// An authoring user.
    User,
    /// A document.
    Document,
    /// A fragment.
    Fragment,
}

/// `ownerships.ownership_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipType {
    /// The owner created the object and is its language parent.
    Origin,
    /// The owner controls the object.
    Owner,
    /// The owner may use the object.
    Member,
}

/// `ownerships.object_type` / `relations.object_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    /// A document.
    Document,
    /// A fragment.
    Fragment,
    /// A text asset.
    Text,
    /// An image asset.
    Image,
}

impl AssetType {
    /// The literal stored in the database column.
    pub fn as_str(self) -> &'static str {
        match self {
            AssetType::Text => "TEXT",
            AssetType::Image => "IMAGE",
        }
    }
}

impl Status {
    /// The literal stored in the database column.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "DRAFT",
            Status::InReview => "IN_REVIEW",
            Status::Released => "RELEASED",
            Status::Deprecated => "DEPRECATED",
        }
    }

    /// Parse a database literal (case-insensitive), e.g. from profile TOML.
    pub fn parse(value: &str) -> Option<Status> {
        match value.trim().to_ascii_uppercase().as_str() {
            "DRAFT" => Some(Status::Draft),
            "IN_REVIEW" => Some(Status::InReview),
            "RELEASED" => Some(Status::Released),
            "DEPRECATED" => Some(Status::Deprecated),
            _ => None,
        }
    }
}

impl OwnerType {
    /// The literal stored in the database column.
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerType::User => "USER",
            OwnerType::Document => "DOCUMENT",
            OwnerType::Fragment => "FRAGMENT",
        }
    }
}

impl OwnershipType {
    /// The literal stored in the database column.
    pub fn as_str(self) -> &'static str {
        match self {
            OwnershipType::Origin => "ORIGIN",
            OwnershipType::Owner => "OWNER",
            OwnershipType::Member => "MEMBER",
        }
    }
}

impl ObjectType {
    /// The literal stored in the database column.
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectType::Document => "DOCUMENT",
            ObjectType::Fragment => "FRAGMENT",
            ObjectType::Text => "TEXT",
            ObjectType::Image => "IMAGE",
        }
    }
}

// ============================================================================
// Business identifiers
// ============================================================================

/// The version this exporter emits for every document and asset variant.
pub const INITIAL_VERSION: i64 = 1;

/// Build the Redacto business identifier for an asset version,
/// e.g. `4420d850-…-b7e9-ver-1`.
///
/// This is the form used in the document configuration, in `relations` and in
/// asset `ownerships` rows — never the technical primary key.
pub fn asset_ref(asset_id: &str, version: i64) -> String {
    format!("{asset_id}-ver-{version}")
}

// ============================================================================
// Table rows
// ============================================================================

/// A row of `app_redacto.assets`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Business identifier referenced by the configuration (UUID, distinct
    /// from [`AssetRow::id`]).
    pub asset_id: String,
    /// Asset kind.
    pub asset_type: AssetType,
}

/// A row of `app_redacto.asset_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetVersionRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Java `Locale.toString()` form, e.g. `de`.
    pub language: String,
    /// Variant version, always [`INITIAL_VERSION`] for generated dumps.
    pub version: i64,
    /// Lifecycle status.
    pub status: Status,
    /// The HTML fragment.
    pub content: String,
    /// Foreign key to [`AssetRow::id`].
    pub asset_fk_id: String,
}

/// A row of `app_redacto.documents`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Business identifier, e.g. `aaad_001`.
    pub document_id: String,
    /// AEM authoring path of the document.
    pub form_path: String,
    /// Document configuration; serialised to JSON only when writing SQL.
    pub configuration: RedactoConfiguration,
}

/// A row of `app_redacto.document_version`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentVersionRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Java `Locale.toString()` form, e.g. `de`.
    pub language: String,
    /// Variant version, always [`INITIAL_VERSION`] for generated dumps.
    pub version: i64,
    /// Lifecycle status.
    pub status: Status,
    /// Foreign key to [`DocumentRow::id`].
    pub document_fk_id: String,
}

/// A row of `app_redacto.ownerships`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Business identifier of the owner.
    pub owner_id: String,
    /// Kind of owner.
    pub owner_type: OwnerType,
    /// Kind of ownership.
    pub ownership_type: OwnershipType,
    /// Business identifier of the owned object.
    pub object_id: String,
    /// Kind of owned object.
    pub object_type: ObjectType,
}

/// A row of `app_redacto.relations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRow {
    /// Technical primary key (UUID).
    pub id: String,
    /// `created` timestamp.
    pub created: String,
    /// Business identifier of the document the object belongs to.
    pub relates_to: String,
    /// Business identifier of the related object.
    pub object_id: String,
    /// Kind of related object.
    pub object_type: ObjectType,
}

// ============================================================================
// Document configuration JSON
// ============================================================================

/// The `documents.configuration` payload (`redacto-document/v1`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactoConfiguration {
    /// Schema marker, `redacto-document/v1`.
    #[serde(rename = "$schema")]
    pub schema: String,
    /// Document metadata.
    pub document: RedactoDocumentMeta,
    /// Ordered component tree.
    pub components: Vec<RedactoComponent>,
}

/// The `document` object inside a [`RedactoConfiguration`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactoDocumentMeta {
    /// Business identifier, mirrors `documents.document_id`.
    pub id: String,
    /// Human-readable title, rendered into the HTML `<title>`.
    pub title: String,
    /// Stylesheet file name resolved from the Redacto bundle, e.g.
    /// `ubs-default.css`.
    pub style: String,
    /// Page header text (`${meta:header}`).
    pub header: String,
    /// Page footer text (`${meta:footer}`).
    pub footer: String,
}

/// A node of the configuration component tree.
///
/// Redacto only deserialises these two component types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RedactoComponent {
    /// A block carrying content, referencing text assets by
    /// [`asset_ref`] identifier.
    AssetContainer {
        /// Component identifier (UUID).
        id: String,
        /// Referenced asset versions, in render order.
        assets: Vec<String>,
    },
    /// A container that groups and styles nested components.
    StyledPanel {
        /// Component identifier.
        id: String,
        /// Space-separated CSS classes, e.g. `layout-split-block`.
        style: String,
        /// Nested components.
        components: Vec<RedactoComponent>,
    },
}

// ============================================================================
// The dump
// ============================================================================

/// A complete set of rows describing one Redacto document.
///
/// Serialise with [`RedactoDump::to_sql`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactoDump {
    /// `app_redacto.assets` rows.
    pub assets: Vec<AssetRow>,
    /// `app_redacto.asset_version` rows.
    pub asset_versions: Vec<AssetVersionRow>,
    /// `app_redacto.documents` rows.
    pub documents: Vec<DocumentRow>,
    /// `app_redacto.document_version` rows.
    pub document_versions: Vec<DocumentVersionRow>,
    /// `app_redacto.ownerships` rows.
    pub ownerships: Vec<OwnershipRow>,
    /// `app_redacto.relations` rows.
    pub relations: Vec<RelationRow>,
    /// Content that could not be represented (input fields, images,
    /// conditionals) or that exceeds a column width.
    pub warnings: Vec<String>,
}

impl RedactoDump {
    /// Total number of `INSERT` statements [`RedactoDump::to_sql`] will emit.
    pub fn row_count(&self) -> usize {
        self.assets.len()
            + self.asset_versions.len()
            + self.documents.len()
            + self.document_versions.len()
            + self.ownerships.len()
            + self.relations.len()
    }
}
