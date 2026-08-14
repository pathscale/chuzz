import type { Layout } from "solid-layouts";
import { page } from "./Page.recipe";

export type PageProps = {
  tabId: string;
  active: boolean;
};

const Page: Layout<typeof page, PageProps> = () => (
  <web-view {...slot.root} id={`chuzz-page-${local.tabId}`} data-tab-id={local.tabId} />
);

export const PageLayout = Page;
export default Page;
