import { useState } from "react";
import "./App.css";
import Sidebar from "./components/Sidebar";
import Toolbar from "./components/Toolbar";
import DownloadList from "./components/DownloadList";
import NewDownloadModal from "./components/NewDownloadModal";

function App() {
  const [isModalOpen, setIsModalOpen] = useState(false);
  
  // Dummy state to visualize categories (not fully functional logic yet)
  const [activeCategory, setActiveCategory] = useState("All Downloads");

  return (
    <div className="app-container">
      <Sidebar activeCategory={activeCategory} onSelectCategory={setActiveCategory} />
      
      <div className="main-content">
        <Toolbar onAddClick={() => setIsModalOpen(true)} />
        <DownloadList category={activeCategory} />
      </div>

      {isModalOpen && (
        <NewDownloadModal onClose={() => setIsModalOpen(false)} />
      )}
    </div>
  );
}

export default App;
