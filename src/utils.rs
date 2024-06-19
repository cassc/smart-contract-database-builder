use regex::Regex;

/// Hashing the content after removing all the whitespaces
pub(crate) fn simple_hash(content: &str) -> String {
    let re = Regex::new(r"\s+").unwrap();
    let result = re.replace_all(content, "");
    let digest = md5::compute(result.as_bytes());
    format!("{:x}", digest)
}
