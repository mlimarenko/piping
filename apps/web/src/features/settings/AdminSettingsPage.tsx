import { Settings2 } from "lucide-react";
import { useTranslation } from "react-i18next";

import SettingsCatalog from "@/features/settings/SettingsCatalog";
import PageChrome from "@/shared/ui-shared/PageChrome";

const AdminSettingsPage = () => {
  const { t } = useTranslation();

  return (
    <div className="flex min-w-0 flex-col gap-6">
      <PageChrome
        title={t("adminSettings.title")}
        description={t("adminSettings.description")}
        icon={Settings2}
      />
      <SettingsCatalog mode="admin" />
    </div>
  );
};

export default AdminSettingsPage;
