import Nav from "@/components/Nav";
import Hero from "@/components/Hero";
import QuickStart from "@/components/QuickStart";
import HowItWorks from "@/components/HowItWorks";
import Features from "@/components/Features";
import Architecture from "@/components/Architecture";
import Footer from "@/components/Footer";

export default function Home() {
  return (
    <>
      <Nav />
      <main>
        <Hero />
        <QuickStart />
        <HowItWorks />
        <Features />
        <Architecture />
      </main>
      <Footer />
    </>
  );
}
