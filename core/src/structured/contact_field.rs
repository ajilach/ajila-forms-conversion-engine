//! Label-driven recognition of email and telephone fields.
//!
//! A form's PDF source carries no type information for these: an email address
//! and a phone number both arrive as plain single-line text (or, worse, as a
//! numeric field whose display clause reformats `+41 44 234 56 78` into
//! `1,234,567.00`). The only signal is the label, so this module turns a label
//! into a [`ContactKind`] and [`super::FieldType::Email`] / [`super::FieldType::Tel`]
//! follow from it.
//!
//! The rules are ported verbatim from the corpus sweep for
//! PROBLEM-email-phone-component (feedback #117, `email_phone_labels.py`), which
//! derived them from 126 instances across 40 packages. Two things about them are
//! deliberate and easy to get wrong when "simplifying":
//!
//! * **Anchoring, not keyword matching.** `e-mail` requires the `e` prefix, which
//!   is what keeps "Mailing address confirmation" out. `mobil` needs a word
//!   boundary on its left, which is what keeps "immobili" / "automobili" out.
//!   `telefon` needs one on its right, which is what keeps "Telefonische
//!   Bestellung" out — an order channel, not a number.
//! * **The deny lists are evidence, not guesswork.** Every entry was seen in the
//!   corpus and checked by hand. "Telefonico canale n." is a channel identifier;
//!   "Titolare / Numero di telefono …" packs a holder name and a number into one
//!   field, so a numeric-only validation clause would reject valid input.
//!
//! Labels in this corpus are mixed German, English, Italian and Spanish, hence
//! the multilingual alternations.

use regex_lite::Regex;
use std::sync::LazyLock;

/// The two contact field kinds a label can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactKind {
    Email,
    Telephone,
}

/// `(?i)` is applied at compile time; `regex-lite` has ASCII word boundaries,
/// which is all these patterns need — every anchor sits next to an ASCII letter.
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(e[-\s.]?mail|courriel|posta\s+elettronica|pec)\b")
        .expect("email label pattern")
});

static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(tel|telefon|telefono|telefonico|telefonica|telephone|t[ée]l[ée]phone|\
tel[eé]fono|phone|mobile|mobil|handy|natel|cellulare|m[oó]vil|fax|telefax|facsimile)\b",
    )
    .expect("phone label pattern")
});

/// Labels that contain a matching word but do not name an email address or a
/// phone number. Matched as a substring of the normalised (lowercased,
/// whitespace-collapsed) label.
const DENY_LABEL: &[&str] = &[
    // "Telefonische Bestellung eines Bundesbankschecks" — an order channel.
    "telefonische bestellung",
    // "Telefonico canale n." — a channel identifier, not a phone number.
    "telefonico canale",
    // Free text combining a holder NAME and a phone number in one field; the
    // numeric-only clause would reject valid input.
    "titolare / numero di telefono",
];

/// Field names whose label is empty or misleading and which are not contact
/// fields.
///
/// Stored bare — without the component-type prefix the corpus spells them with
/// (`TXT_MailingAddressConfirmation`) — because this classifier runs on the
/// structured model, where a field is named by its SOM leaf and no such prefix
/// exists yet. [`strip_type_prefix`] reconciles the two, so an entry matches
/// whichever form it is given. Matched as a prefix, so the engine's trailing
/// `_<short-uuid>` disambiguator is irrelevant.
const DENY_NAME: &[&str] = &[
    "MailingAddressConfirmation",
    "Amountimobili",
    "TotaleImmobili",
];

/// Drop a leading component-type prefix (`TXT_`, `NB_`, `DD_`, …) from an AEM
/// component name. A SOM leaf carries none and is returned unchanged.
fn strip_type_prefix(name: &str) -> &str {
    match name.split_once('_') {
        Some((prefix, rest))
            if !prefix.is_empty()
                && prefix.len() <= 4
                && prefix.chars().all(|c| c.is_ascii_uppercase()) =>
        {
            rest
        }
        _ => name,
    }
}

/// Collapse runs of whitespace and trim, the shape the deny lists are written in.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Classify a field from its label, falling back to its name when it has none.
///
/// `name` is only consulted for untitled fields. In the corpus all 386 untitled
/// text boxes are letterhead/address slots and none of them classify, so the
/// fallback costs nothing and covers the case where a label was never attached.
///
/// Returns `None` for anything that is not unambiguously an email or a phone
/// number — the conservative direction, since the wrong answer here attaches a
/// strict validation clause to a field that cannot satisfy it.
pub fn classify(label: Option<&str>, name: &str) -> Option<ContactKind> {
    let bare_name = strip_type_prefix(name);
    let bare_lower = bare_name.to_lowercase();
    if DENY_NAME
        .iter()
        .any(|d| bare_lower.starts_with(&d.to_lowercase()))
    {
        return None;
    }

    let label = normalize(label.unwrap_or_default());
    let lowered = label.to_lowercase();
    if DENY_LABEL.iter().any(|d| lowered.contains(d)) {
        return None;
    }

    // An untitled field is probed through its name with separators turned back
    // into spaces, so `TXT_Email_Private` reads as a label would.
    let probe = if label.is_empty() {
        normalize(&bare_name.replace('_', " "))
    } else {
        label
    };

    if EMAIL_RE.is_match(&probe) {
        Some(ContactKind::Email)
    } else if PHONE_RE.is_match(&probe) {
        Some(ContactKind::Telephone)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(label: &str) -> Option<ContactKind> {
        classify(Some(label), "TXT_Some_1a2b")
    }

    #[test]
    fn recognizes_email_labels_across_languages() {
        for label in [
            "E-Mail",
            "E-Mail-Adresse",
            "Email address",
            "e mail",
            "Indirizzo di posta elettronica",
            "Courriel",
            "PEC",
        ] {
            assert_eq!(kind(label), Some(ContactKind::Email), "label {label:?}");
        }
    }

    #[test]
    fn recognizes_phone_labels_across_languages() {
        for label in [
            "Telefon",
            "Telefon-Nr.",
            "Tel. Nr.",
            "Telephone",
            "Téléphone",
            "Numero di telefono",
            "Mobile phone",
            "Handy",
            "Natel",
            "Cellulare",
            "Móvil",
            "Fax",
            "Telefax",
        ] {
            assert_eq!(kind(label), Some(ContactKind::Telephone), "label {label:?}");
        }
    }

    /// The anchoring cases. Each of these matched a naive `contains` and was a
    /// real false positive in the corpus.
    #[test]
    fn anchoring_keeps_near_misses_out() {
        for label in [
            // `e-mail` needs the `e` prefix.
            "Mailing address confirmation",
            // A letter precedes `mobil`.
            "Totale immobili",
            "Automobili",
            // The right boundary fails before `ische`.
            "Telefonische Bestellung",
            // Deny-listed: a channel identifier, not a number.
            "Telefonico canale n.",
            // Deny-listed: holder name and number in one field.
            "Titolare / Numero di telefono in caso di domande",
            // No contact word at all.
            "Vorname",
            "Strasse und Hausnummer",
        ] {
            assert_eq!(kind(label), None, "label {label:?}");
        }
    }

    /// A closed compound swallows the boundary, so `Telefonnummer` does not
    /// match where `Telefon-Nr.` does. That is the sweep's own behaviour, not an
    /// oversight in this port: the same right boundary is what keeps
    /// "Telefonische Bestellung" out, and the corpus's 126 instances were
    /// classified under exactly these rules. Widening it needs new evidence and
    /// a re-run of the near-miss audit, not a quiet edit here.
    #[test]
    fn closed_compounds_do_not_match() {
        assert_eq!(kind("Telefonnummer privat"), None);
        assert_eq!(kind("Mobiltelefon"), None);
    }

    /// The deny list is written bare so it matches both the AEM component name
    /// the corpus uses and the SOM leaf this classifier actually sees.
    #[test]
    fn deny_named_fields_never_classify() {
        assert_eq!(
            classify(Some("E-Mail"), "TXT_MailingAddressConfirmation_9f21"),
            None
        );
        assert_eq!(classify(Some("E-Mail"), "MailingAddressConfirmation"), None);
    }

    #[test]
    fn type_prefixes_are_stripped_but_ordinary_names_survive() {
        assert_eq!(strip_type_prefix("TXT_Email_Private"), "Email_Private");
        assert_eq!(strip_type_prefix("NB_Fax"), "Fax");
        // Not a type prefix: lowercase, or too long to be one.
        assert_eq!(strip_type_prefix("form_Email"), "form_Email");
        assert_eq!(strip_type_prefix("SENDER_Email"), "SENDER_Email");
        assert_eq!(strip_type_prefix("Email"), "Email");
    }

    #[test]
    fn falls_back_to_the_name_when_untitled() {
        assert_eq!(
            classify(None, "TXT_Email_Private_9f21"),
            Some(ContactKind::Email)
        );
        assert_eq!(
            classify(Some("   "), "TXT_Telefon_1a2b"),
            Some(ContactKind::Telephone)
        );
        // A label, once present, wins over the name.
        assert_eq!(classify(Some("Vorname"), "TXT_Telefon_1a2b"), None);
    }

    #[test]
    fn email_wins_over_phone_when_a_label_carries_both() {
        // "E-Mail / Telefon" is one field in several forms; the email clause is
        // the permissive one, so it is the safe answer.
        assert_eq!(kind("E-Mail / Telefon"), Some(ContactKind::Email));
    }
}
