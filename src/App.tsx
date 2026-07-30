import { useState, useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import Sidebar from "./components/Sidebar";
import Toolbar from "./components/Toolbar";
import DownloadList from "./components/DownloadList";
import NewDownloadModal from "./components/NewDownloadModal";

function App() {
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [prefilledUrl, setPrefilledUrl] = useState("");
  
  // Dummy state to visualize categories (not fully functional logic yet)
  const [activeCategory, setActiveCategory] = useState("All Downloads");

  useEffect(() => {
    const unlisten = listen<{ url: string }>("intercepted-media", (event) => {
      setPrefilledUrl(event.payload.url);
      setIsModalOpen(true);
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  return (
    <div className="app-container">
      <Sidebar activeCategory={activeCategory} onSelectCategory={setActiveCategory} />
      
      <div className="main-content">
        <Toolbar onAddClick={() => setIsModalOpen(true)} />
        <DownloadList category={activeCategory} />
      </div>

      {isModalOpen && (
        <NewDownloadModal 
          onClose={() => {
            setIsModalOpen(false);
            setPrefilledUrl("");
          }}
          onSuccess={() => {
            // Trigger a refetch in DownloadList by slightly changing activeCategory 
            // or we just let the events handle it
            setActiveCategory(prev => prev === "All Downloads" ? "All Downloads " : "All Downloads");
          }}
          initialUrl={prefilledUrl}
        />
      )}
    </div>
  );
}

export default App;
