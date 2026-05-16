// In-tree 3D force-graph entry point. Side-effect CSS import matches
// upstream's index.js so bundlers walking re-exports still pick up
// the container + info-banner styles.
import "./styles.css";
export { default } from "./kapsule";
