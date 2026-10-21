import { Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import SettingsCatalog from "@/features/settings/SettingsCatalog";
import PageChrome from "@/shared/ui-shared/PageChrome";
import PageLayout from "@/shared/ui-shared/PageLayout";

const AdminSettingsPage = () => {
  const { t } = useTranslation();

  return (
    <PageLayout width="form" className="flex flex-col gap-6">
      <PageChrome
        title={t("adminSettings.title")}
        description={t("adminSettings.description")}
        icon={Settings2}
      />
      <SettingsCatalog mode="admin" />
    </PageLayout>
  );
};

export default AdminSettingsPage;
