import ComponentTypes from '@theme-original/NavbarItem/ComponentTypes';
import GitHubStars from '@site/src/components/GitHubStars';

// Register a new navbar item type so the classic theme's NavbarItem
// dispatcher can render <GitHubStars /> when it sees
// `type: 'custom-githubStars'` in docusaurus.config.js.
export default {
  ...ComponentTypes,
  'custom-githubStars': GitHubStars,
};
