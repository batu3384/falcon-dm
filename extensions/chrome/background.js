// Falcon DM Chrome Extension Background Script

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "download_with_falcon",
    title: "Download with Falcon DM",
    contexts: ["link", "image", "video", "audio"]
  });
});

async function sendToFalcon(downloadItem) {
  let cookiesHeader = "";
  try {
    const cookies = await chrome.cookies.getAll({ url: downloadItem.url });
    cookiesHeader = cookies.map(c => `${c.name}=${c.value}`).join("; ");
  } catch (e) {
    console.error("Failed to get cookies:", e);
  }

  const payload = {
    url: downloadItem.url,
    filename: downloadItem.filename,
    referrer: downloadItem.referrer || "",
    user_agent: navigator.userAgent,
    cookies: cookiesHeader
  };

  try {
    const response = await fetch("http://127.0.0.1:14201/api/add", {
      method: "POST",
      headers: {
        "Content-Type": "application/json"
      },
      body: JSON.stringify(payload)
    });
    
    if (!response.ok) {
      throw new Error(`HTTP error! status: ${response.status}`);
    }
    console.log("Successfully sent download to Falcon DM.");
  } catch (error) {
    console.error("Failed to send download to Falcon DM:", error);
  }
}

chrome.downloads.onDeterminingFilename.addListener((item, suggest) => {
  // Cancel browser download
  suggest({ cancel: true });

  // Send to Falcon DM
  sendToFalcon({
    url: item.url,
    filename: item.filename,
    referrer: item.referrer
  });
});

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === "download_with_falcon") {
    let url = info.linkUrl || info.srcUrl;
    if (url) {
      // Default to empty filename, the backend/aria2 will determine it if possible
      let filename = url.split('/').pop().split('?')[0] || "download";
      sendToFalcon({
        url: url,
        filename: filename,
        referrer: info.pageUrl
      });
    }
  }
});
