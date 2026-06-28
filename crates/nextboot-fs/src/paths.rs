use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// 路径规范化
pub fn normalize_path(path: &str) -> String {
    let mut result = String::new();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    for part in parts {
        if part == "." {
            continue;
        }
        if part == ".." {
            // 简化处理，不支持 ..
            continue;
        }
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(part);
    }

    if result.is_empty() {
        String::from("/")
    } else {
        result
    }
}

/// 分割路径为目录和文件名
pub fn split_path(path: &str) -> (String, String) {
    let normalized = normalize_path(path);
    if let Some(pos) = normalized.rfind('/') {
        let dir = &normalized[..pos];
        let name = &normalized[pos + 1..];
        (
            if dir.is_empty() {
                String::from("/")
            } else {
                dir.to_string()
            },
            name.to_string(),
        )
    } else {
        (String::from("/"), normalized)
    }
}
