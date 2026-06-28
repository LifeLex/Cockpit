import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

function App() {
  const [version, setVersion] = useState<string>("");

  useEffect(() => {
    invoke<string>("get_version").then((v) => {
      setVersion(v);
    });
  }, []);

  return (
    <main>
      <h1>Cockpit</h1>
      <p>Version: {version}</p>
    </main>
  );
}

export default App;
