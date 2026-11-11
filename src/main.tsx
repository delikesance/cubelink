import { createRoot } from "react-dom/client";
import "./styles/tokens.css";
import { Shell } from "./app/shell";
import { installExternalLinks } from "./app/externalLinks";

installExternalLinks();
createRoot(document.getElementById("root")!).render(<Shell />);
