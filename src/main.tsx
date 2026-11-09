import { createRoot } from "react-dom/client";
import "./styles/tokens.css";
import { Shell } from "./app/shell";

createRoot(document.getElementById("root")!).render(<Shell />);
