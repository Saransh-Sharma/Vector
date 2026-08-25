import { create } from "zustand";

type UiState = {
  active: string;
  paletteOpen: boolean;
  onboardingStep: number;
  setActive: (active: string) => void;
  setPaletteOpen: (open: boolean) => void;
  setOnboardingStep: (step: number) => void;
};

export const useUi = create<UiState>((set) => ({
  active: "Cockpit",
  paletteOpen: false,
  onboardingStep: Number(localStorage.getItem("vector.onboarding.step") ?? 0),
  setActive: (active) => set({ active }),
  setPaletteOpen: (paletteOpen) => set({ paletteOpen }),
  setOnboardingStep: (onboardingStep) => {
    localStorage.setItem("vector.onboarding.step", String(onboardingStep));
    set({ onboardingStep });
  },
}));

