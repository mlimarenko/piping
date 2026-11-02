import { useBranding } from "@/app/BrandingContext";

const PipingLogo = ({ size = 28, className = "" }: { size?: number; className?: string }) => {
  const { logoDataUrl, appName } = useBranding();

  if (logoDataUrl) {
    return (
      <img
        src={logoDataUrl}
        alt={appName}
        width={size}
        height={size}
        className={className}
        style={{ width: size, height: size, objectFit: "contain" }}
      />
    );
  }

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
    >
      <defs>
        <mask id="mask-large-logo">
          <rect width="32" height="32" fill="black" />
          <circle cx="12" cy="20" r="12" fill="white" />
          <circle cx="20" cy="12" r="9" fill="black" />
        </mask>
        <mask id="mask-small-logo">
          <rect width="32" height="32" fill="black" />
          <circle cx="20" cy="12" r="9" fill="white" />
          <circle cx="12" cy="20" r="12" fill="black" />
        </mask>
      </defs>
      <circle cx="12" cy="20" r="12" fill="hsl(var(--primary-fill))" mask="url(#mask-large-logo)" />
      <circle cx="20" cy="12" r="9" fill="hsl(var(--primary-fill))" mask="url(#mask-small-logo)" />
    </svg>
  );
};

export default PipingLogo;
