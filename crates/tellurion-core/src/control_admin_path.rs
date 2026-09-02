//! Canonical control-plane paths and anchored segment patterns.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlPathError {
    InvalidUtf8,
    OutsideApplicationRoot,
    EncodedComponent,
    InvalidSeparator,
    RepeatedSeparator,
    DotSegment,
    TrailingSeparator,
    InvalidHierarchy,
    InvalidPattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalControlPath {
    value: String,
    segments: Vec<String>,
}

impl CanonicalControlPath {
    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn segments(&self) -> impl ExactSizeIterator<Item = &str> {
        self.segments.iter().map(String::as_str)
    }
}

pub fn canonicalize_control_path(
    raw_path: &[u8],
    application_root: &str,
) -> Result<CanonicalControlPath, ControlPathError> {
    let raw_path = std::str::from_utf8(raw_path).map_err(|_| ControlPathError::InvalidUtf8)?;
    let path = strip_application_root(raw_path, application_root)?;
    validate_unambiguous_path(path)?;
    let segments = path[1..].split('/').map(str::to_string).collect::<Vec<_>>();
    validate_hierarchy(&segments)?;
    Ok(CanonicalControlPath {
        value: path.to_string(),
        segments,
    })
}

fn strip_application_root<'a>(
    raw_path: &'a str,
    application_root: &str,
) -> Result<&'a str, ControlPathError> {
    if application_root.is_empty() || application_root == "/" {
        return Ok(raw_path);
    }
    if !application_root.starts_with('/')
        || application_root.ends_with('/')
        || application_root.contains('%')
        || application_root.contains('\\')
    {
        return Err(ControlPathError::OutsideApplicationRoot);
    }
    raw_path
        .strip_prefix(application_root)
        .filter(|path| path.starts_with('/'))
        .ok_or(ControlPathError::OutsideApplicationRoot)
}

fn validate_unambiguous_path(path: &str) -> Result<(), ControlPathError> {
    if !path.starts_with('/') {
        return Err(ControlPathError::InvalidHierarchy);
    }
    if path.contains('%') {
        return Err(ControlPathError::EncodedComponent);
    }
    if path.contains('\\') {
        return Err(ControlPathError::InvalidSeparator);
    }
    if path.contains("//") {
        return Err(ControlPathError::RepeatedSeparator);
    }
    if path.len() > 1 && path.ends_with('/') {
        return Err(ControlPathError::TrailingSeparator);
    }
    if path[1..]
        .split('/')
        .any(|segment| matches!(segment, "." | ".."))
    {
        return Err(ControlPathError::DotSegment);
    }
    Ok(())
}

fn validate_hierarchy(segments: &[String]) -> Result<(), ControlPathError> {
    if segments.len() < 3 || segments[0] != "_control" || segments[1] != "v1" {
        return Err(ControlPathError::InvalidHierarchy);
    }
    match segments[2].as_str() {
        "platform" | "tenants" => Ok(()),
        _ => Err(ControlPathError::InvalidHierarchy),
    }
}

pub(crate) fn validate_inert_legacy_pattern(pattern: &str) -> Result<(), ControlPathError> {
    if pattern.starts_with('/') {
        Ok(())
    } else {
        Err(ControlPathError::InvalidPattern)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternSegment {
    Literal(String),
    One,
    Remainder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPathPattern {
    segments: Vec<PatternSegment>,
}

impl CompiledPathPattern {
    pub fn compile(pattern: &str) -> Result<Self, ControlPathError> {
        validate_unambiguous_path(pattern).map_err(|_| ControlPathError::InvalidPattern)?;
        let raw_segments = pattern[1..].split('/').collect::<Vec<_>>();
        if raw_segments.len() < 3
            || raw_segments[0] != "_control"
            || raw_segments[1] != "v1"
            || !matches!(raw_segments[2], "platform" | "tenants" | "*" | "**")
        {
            return Err(ControlPathError::InvalidPattern);
        }
        let mut segments = Vec::with_capacity(raw_segments.len());
        for (index, segment) in raw_segments.iter().enumerate() {
            let compiled = match *segment {
                "*" => PatternSegment::One,
                "**" if index + 1 == raw_segments.len() => PatternSegment::Remainder,
                "**" => return Err(ControlPathError::InvalidPattern),
                literal if literal.contains('*') || literal.is_empty() => {
                    return Err(ControlPathError::InvalidPattern)
                }
                literal => PatternSegment::Literal(literal.to_string()),
            };
            segments.push(compiled);
        }
        Ok(Self { segments })
    }

    pub fn matches(&self, path: &CanonicalControlPath) -> bool {
        let path = &path.segments;
        for (index, segment) in self.segments.iter().enumerate() {
            match segment {
                PatternSegment::Remainder => return true,
                PatternSegment::One if path.get(index).is_some() => {}
                PatternSegment::Literal(expected)
                    if path.get(index).is_some_and(|actual| actual == expected) => {}
                _ => return false,
            }
        }
        self.segments.len() == path.len()
    }

    pub(crate) fn covers(&self, other: &Self) -> bool {
        for (index, segment) in self.segments.iter().enumerate() {
            let Some(other_segment) = other.segments.get(index) else {
                return matches!(segment, PatternSegment::Remainder);
            };
            match (segment, other_segment) {
                (PatternSegment::Remainder, _) => return true,
                (PatternSegment::One, PatternSegment::Literal(_) | PatternSegment::One) => {}
                (PatternSegment::Literal(left), PatternSegment::Literal(right))
                    if left == right => {}
                _ => return false,
            }
        }
        self.segments.len() == other.segments.len()
    }

    pub(crate) fn overlaps(&self, other: &Self) -> bool {
        let max = self.segments.len().max(other.segments.len());
        for index in 0..max {
            match (self.segments.get(index), other.segments.get(index)) {
                (Some(PatternSegment::Remainder), _) | (_, Some(PatternSegment::Remainder)) => {
                    return true
                }
                (Some(PatternSegment::Literal(left)), Some(PatternSegment::Literal(right)))
                    if left != right =>
                {
                    return false
                }
                (Some(_), Some(_)) => {}
                (None, None) => return true,
                _ => return false,
            }
        }
        true
    }

    pub(crate) fn intersection(&self, other: &Self) -> Option<Self> {
        let mut segments = Vec::new();
        let mut index = 0;
        loop {
            match (self.segments.get(index), other.segments.get(index)) {
                (Some(PatternSegment::Remainder), Some(_)) => {
                    segments.extend_from_slice(&other.segments[index..]);
                    return Some(Self { segments });
                }
                (Some(_), Some(PatternSegment::Remainder)) => {
                    segments.extend_from_slice(&self.segments[index..]);
                    return Some(Self { segments });
                }
                (Some(PatternSegment::Literal(left)), Some(PatternSegment::Literal(right))) => {
                    if left != right {
                        return None;
                    }
                    segments.push(PatternSegment::Literal(left.clone()));
                }
                (Some(PatternSegment::Literal(value)), Some(PatternSegment::One))
                | (Some(PatternSegment::One), Some(PatternSegment::Literal(value))) => {
                    segments.push(PatternSegment::Literal(value.clone()));
                }
                (Some(PatternSegment::One), Some(PatternSegment::One)) => {
                    segments.push(PatternSegment::One);
                }
                (None, None) => return Some(Self { segments }),
                (None, Some(PatternSegment::Remainder))
                | (Some(PatternSegment::Remainder), None) => return Some(Self { segments }),
                (None, Some(_)) | (Some(_), None) => return None,
            }
            index += 1;
        }
    }
}
