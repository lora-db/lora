import React from "react";
import OriginalCodeBlock from "@theme-original/CodeBlock";
import LoraQueryCodeBlock from "@site/src/components/LoraQueryCodeBlock";

function languageFromProps({ language, className }) {
  if (language) return language;
  return className?.match(/(?:^|\s)language-([^\s]+)/)?.[1];
}

export default function CodeBlock(props) {
  const detectedLanguage = languageFromProps(props);

  if (detectedLanguage === "cypher" && typeof props.children === "string") {
    return <LoraQueryCodeBlock {...props} />;
  }

  return <OriginalCodeBlock {...props} />;
}
