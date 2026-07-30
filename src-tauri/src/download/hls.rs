use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use m3u8_rs::Playlist;
use url::Url;
use reqwest::Client;
use futures::stream::{self, StreamExt};

pub async fn process_hls_stream(url: &str, save_path: &str) -> Result<(), String> {
    let client = Client::new();
    let base_url = Url::parse(url).map_err(|e| e.to_string())?;

    // 1. Fetch manifest
    let res = client.get(url).send().await.map_err(|e| e.to_string())?;
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    
    // 2. Parse manifest
    let playlist = match m3u8_rs::parse_playlist_res(&bytes) {
        Ok(p) => p,
        Err(_) => return Err("Failed to parse m3u8 playlist".into()),
    };

    let mut segment_urls = Vec::new();

    match playlist {
        Playlist::MasterPlaylist(pl) => {
            // Pick highest quality stream (or just the first one for simplicity)
            if let Some(variant) = pl.variants.first() {
                let variant_url = base_url.join(&variant.uri).map_err(|e| e.to_string())?;
                // Fetch this media playlist
                let m_res = client.get(variant_url.clone()).send().await.map_err(|e| e.to_string())?;
                let m_bytes = m_res.bytes().await.map_err(|e| e.to_string())?;
                match m3u8_rs::parse_playlist_res(&m_bytes) {
                    Ok(Playlist::MediaPlaylist(m_pl)) => {
                        for seg in m_pl.segments {
                            segment_urls.push(variant_url.join(&seg.uri).map_err(|e| e.to_string())?);
                        }
                    },
                    _ => return Err("Expected MediaPlaylist in variant".into()),
                }
            } else {
                return Err("No variants found in master playlist".into());
            }
        },
        Playlist::MediaPlaylist(pl) => {
            for seg in pl.segments {
                segment_urls.push(base_url.join(&seg.uri).map_err(|e| e.to_string())?);
            }
        },
    }

    if segment_urls.is_empty() {
        return Err("No segments found".into());
    }

    // 3. Create temp dir
    let out_path = Path::new(save_path);
    let parent = out_path.parent().unwrap_or_else(|| Path::new("."));
    let temp_dir = parent.join(format!("{}-{}.falcondm-temp", out_path.file_name().unwrap_or_default().to_string_lossy(), uuid::Uuid::new_v4()));
    
    if !temp_dir.exists() {
        fs::create_dir_all(&temp_dir).await.map_err(|e| e.to_string())?;
    }

    // 4. Download segments concurrently
    let concurrency_limit = 10;
    
    let segment_paths: Vec<PathBuf> = stream::iter(segment_urls.into_iter().enumerate())
        .map(|(idx, seg_url)| {
            let client = client.clone();
            let temp_dir = temp_dir.clone();
            async move {
                let seg_path = temp_dir.join(format!("seg_{:05}.ts", idx));
                let res = client.get(seg_url).send().await.map_err(|e| e.to_string())?;
                let bytes = res.bytes().await.map_err(|e| e.to_string())?;
                let mut file = fs::File::create(&seg_path).await.map_err(|e| e.to_string())?;
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
                Ok::<PathBuf, String>(seg_path)
            }
        })
        .buffer_unordered(concurrency_limit)
        .collect::<Vec<Result<PathBuf, String>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<PathBuf>, String>>()?;

    // Sort paths since they might complete out of order
    let mut segment_paths = segment_paths;
    segment_paths.sort();

    // 5. Merge with FFmpeg
    let segments_txt_path = temp_dir.join("segments.txt");
    let mut txt_file = fs::File::create(&segments_txt_path).await.map_err(|e| e.to_string())?;
    for p in &segment_paths {
        let line = format!("file '{}'\n", p.file_name().unwrap().to_string_lossy());
        txt_file.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    
    let output = Command::new("ffmpeg")
        .current_dir(&temp_dir)
        .arg("-y")
        .arg("-f").arg("concat")
        .arg("-safe").arg("0")
        .arg("-i").arg("segments.txt")
        .arg("-c").arg("copy")
        .arg(save_path)
        .output()
        .await
        .map_err(|e| format!("FFmpeg failed to start: {}", e))?;

    if !output.status.success() {
        return Err(format!("FFmpeg failed: {}", String::from_utf8_lossy(&output.stderr)));
    }

    // 6. Cleanup temp dir
    let _ = fs::remove_dir_all(temp_dir).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dummy_hls_parse() {
        let m3u8 = b"#EXTM3U\n#EXT-X-VERSION:3\n#EXTINF:10.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let parsed = m3u8_rs::parse_playlist_res(m3u8);
        assert!(parsed.is_ok());
    }
}
