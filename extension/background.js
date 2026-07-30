const FALCON_DM_API = "http://127.0.0.1:14201/api/intercept";
const TARGET_EXTENSIONS = [".m3u8", ".mp4", ".ts", ".flv", ".webm", ".mkv", ".mp3", ".wav"];

// Keep track of recently intercepted URLs to avoid spamming the app
const recentIntercepts = new Set();

chrome.webRequest.onBeforeRequest.addListener(
  (details) => {
    // Only intercept main_frame or media requests, or xmlhttprequests that look like media
    const url = details.url.toLowerCase();
    
    let isMedia = false;
    for (const ext of TARGET_EXTENSIONS) {
      if (url.includes(ext)) {
        isMedia = true;
        break;
      }
    }

    if (isMedia && !recentIntercepts.has(url)) {
      recentIntercepts.add(url);
      
      // Clear from cache after 10 seconds to allow re-intercept
      setTimeout(() => recentIntercepts.delete(url), 10000);

      // Send to Falcon DM local server
      fetch(FALCON_DM_API, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          url: details.url,
          page_url: details.initiator || details.documentUrl || "Unknown",
          media_type: details.type
        }),
      })
      .then(response => response.json())
      .then(data => {
        if (data.success) {
          chrome.notifications.create({
            type: "basic",
            iconUrl: "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAxMDAgMTAwIj48Y2lyY2xlIGN4PSI1MCIgY3k9IjUwIiByPSI1MCIgZmlsbD0iIzAwN2FmZiIvPjwvc3ZnPg==",
            title: "Falcon DM",
            message: "Media captured! Check Falcon DM to download."
          });
        }
      })
      .catch(err => console.log("Falcon DM is not running or unreachable:", err));
    }
  },
  { urls: ["<all_urls>"] }
);
