import { useState } from 'react';

interface OnboardingProps {
  onComplete: () => void;
}

export const OnboardingWizard = ({ onComplete }: OnboardingProps) => {
  const [step, setStep] = useState(1);

  const finish = () => {
    localStorage.setItem('onboarding_complete', 'true');
    onComplete();
  };

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-md text-foreground">
      <div className="bg-background w-full max-w-2xl rounded-2xl shadow-2xl overflow-hidden border border-border/50 flex flex-col relative animate-fade-in-up">
        
        {/* Header */}
        <div className="bg-primary/5 p-8 text-center border-b border-border/50">
          <div className="mx-auto w-20 h-20 bg-primary/10 rounded-2xl flex items-center justify-center mb-4 shadow-inner">
            <span className="text-4xl">🦅</span>
          </div>
          <h1 className="text-2xl font-bold tracking-tight text-primary">Welcome to Falcon DM</h1>
          <p className="text-muted-foreground mt-2">The ultimate macOS download manager and media sniffer.</p>
        </div>

        {/* Content */}
        <div className="p-8">
          {step === 1 && (
            <div className="space-y-4 animate-fade-in">
              <h2 className="text-xl font-semibold flex items-center gap-2">
                <span className="bg-primary text-primary-foreground w-6 h-6 rounded-full flex items-center justify-center text-sm">1</span>
                Browser Integration (Crucial)
              </h2>
              <p className="text-muted-foreground leading-relaxed">
                To capture hidden HLS streams (m3u8), videos, and regular downloads automatically like IDM, you <b>must</b> install the Falcon Sniffer browser extension.
              </p>
              
              <div className="bg-muted/50 p-4 rounded-xl border border-border/50 font-mono text-sm space-y-2">
                <p>1. Open Chrome or Edge and go to <code className="bg-background px-1 py-0.5 rounded">chrome://extensions</code></p>
                <p>2. Enable <strong>Developer Mode</strong> in the top right corner.</p>
                <p>3. Click <strong>Load unpacked</strong> and select the <code className="bg-background px-1 py-0.5 rounded text-primary">extension/</code> folder in this project.</p>
              </div>
            </div>
          )}

          {step === 2 && (
            <div className="space-y-4 animate-fade-in">
              <h2 className="text-xl font-semibold flex items-center gap-2">
                <span className="bg-primary text-primary-foreground w-6 h-6 rounded-full flex items-center justify-center text-sm">2</span>
                You're All Set!
              </h2>
              <p className="text-muted-foreground leading-relaxed">
                Falcon DM is now running a secure local server (IPC) on port 14201. 
                Whenever the extension detects a media stream or file, it will instantly pop up a download prompt here!
              </p>
              <div className="flex justify-center py-4">
                <div className="w-16 h-16 bg-green-500/10 rounded-full flex items-center justify-center text-green-500 shadow-[0_0_15px_rgba(34,197,94,0.3)]">
                  <svg className="w-8 h-8" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M5 13l4 4L19 7"></path></svg>
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="p-6 bg-muted/30 border-t border-border/50 flex justify-between items-center">
          <div className="flex space-x-1">
            <div className={`w-2 h-2 rounded-full ${step === 1 ? 'bg-primary' : 'bg-primary/20'}`} />
            <div className={`w-2 h-2 rounded-full ${step === 2 ? 'bg-primary' : 'bg-primary/20'}`} />
          </div>
          
          <div className="space-x-3">
            {step > 1 && (
              <button 
                onClick={() => setStep(step - 1)}
                className="px-5 py-2.5 rounded-lg text-sm font-medium text-foreground bg-transparent hover:bg-muted transition-colors"
              >
                Back
              </button>
            )}
            
            {step < 2 ? (
              <button 
                onClick={() => setStep(step + 1)}
                className="px-6 py-2.5 rounded-lg text-sm font-medium bg-primary text-primary-foreground hover:bg-primary/90 transition-all shadow-md hover:shadow-lg active:scale-95"
              >
                Next Step
              </button>
            ) : (
              <button 
                onClick={finish}
                className="px-6 py-2.5 rounded-lg text-sm font-medium bg-green-600 text-white hover:bg-green-700 transition-all shadow-[0_0_15px_rgba(22,163,74,0.4)] active:scale-95"
              >
                Start Downloading
              </button>
            )}
          </div>
        </div>
        
      </div>
    </div>
  );
};
