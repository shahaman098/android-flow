import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import Bubble from "./Bubble";
import Hub from "./Hub";
import "./App.css";

export default function App() {
  const [label, setLabel] = useState<string | null>(null);

  useEffect(() => {
    setLabel(getCurrentWindow().label);
  }, []);

  if (!label) return null;
  return label === "bubble" ? <Bubble /> : <Hub />;
}
