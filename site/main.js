// GSAP + ScrollTrigger + Lenis, en vanilla : Lenis suit la boucle de GSAP.
(() => {
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const hasGsap = typeof gsap !== 'undefined' && typeof ScrollTrigger !== 'undefined';

  let lenis = null;
  if (hasGsap) {
    gsap.registerPlugin(ScrollTrigger);
    if (!reduced && typeof Lenis !== 'undefined') {
      lenis = new Lenis({ autoRaf: false, lerp: 0.1 });
      lenis.on('scroll', ScrollTrigger.update);
      gsap.ticker.add((t) => lenis.raf(t * 1000));
      gsap.ticker.lagSmoothing(0);
    }
  }

  // Les ancres passent par Lenis, sinon le saut casse le lissage.
  document.querySelectorAll('a[href^="#"]').forEach((a) => {
    a.addEventListener('click', (e) => {
      const target = document.querySelector(a.getAttribute('href'));
      if (!target) return;
      e.preventDefault();
      if (lenis) lenis.scrollTo(target, { offset: -64 });
      else target.scrollIntoView({ behavior: reduced ? 'auto' : 'smooth' });
    });
  });

  // La barre du haut prend un fond une fois le héros dépassé.
  const bar = document.querySelector('.bar');
  const onScroll = () => bar.classList.toggle('is-scrolled', scrollY > innerHeight * 0.9);
  addEventListener('scroll', onScroll, { passive: true });
  onScroll();

  // Le journal : « Annuler » raye l'entrée, comme dans l'app.
  document.querySelectorAll('.journal__e .pill--btn').forEach((b) => {
    b.addEventListener('click', () => {
      const e = b.closest('.journal__e');
      const undone = e.classList.toggle('is-undone');
      b.textContent = undone ? 'Rétablir' : 'Annuler';
    });
  });

  // Copier les commandes d'installation.
  const copy = document.querySelector('.term__copy');
  copy?.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(copy.dataset.copy);
      copy.textContent = 'Copié';
    } catch {
      copy.textContent = 'Sélectionnez le texte';
    }
    setTimeout(() => (copy.textContent = 'Copier'), 1600);
  });

  if (!hasGsap) return;

  // Le logotype : le balai entre, « y », « s », « e » s'envolent
  // aux places du document de design (décalage en F, rotation, opacité).
  const FLYING = [
    { x: 0, y: 0, r: 10, o: 1 },
    { x: 0.088, y: -0.294, r: 30, o: 0.7 },
    { x: 0.206, y: -0.706, r: 60, o: 0.4 },
  ];
  const logo = document.querySelector('.logo');
  const broom = logo.querySelector('.logo__broom');
  const broomSvg = broom.querySelector('svg');
  const letters = gsap.utils.toArray('.logo__l');
  const F = () => parseFloat(getComputedStyle(logo).fontSize);
  const gap = 0.147 + 0.853 + 0.059;

  const finalLogo = () => {
    gsap.set(broom, { width: () => gap * F() });
    gsap.set(broomSvg, { y: 0, rotation: 0, autoAlpha: 1 });
    letters.forEach((l, i) => gsap.set(l, { x: FLYING[i].x * F(), y: FLYING[i].y * F(), rotation: FLYING[i].r, opacity: FLYING[i].o }));
  };

  const mm = gsap.matchMedia();

  mm.add('(prefers-reduced-motion: reduce)', () => {
    finalLogo();
    addEventListener('resize', finalLogo);
    return () => removeEventListener('resize', finalLogo);
  });

  mm.add('(prefers-reduced-motion: no-preference)', () => {
    const tl = gsap.timeline({
      defaults: { ease: 'none' },
      scrollTrigger: {
        trigger: '.hero',
        start: 'top top',
        end: '+=110%',
        pin: '.hero__pin',
        scrub: 0.6,
        invalidateOnRefresh: true,
      },
    });
    tl.fromTo(broomSvg, { y: () => -1.5 * F(), rotation: -70, autoAlpha: 0 }, { y: 0, rotation: 0, autoAlpha: 1, duration: 0.45, ease: 'power2.out' }, 0)
      .fromTo(broom, { width: 0 }, { width: () => gap * F(), duration: 0.45, ease: 'power2.inOut' }, 0)
      .to(broomSvg, { rotation: 14, duration: 0.15, ease: 'sine.inOut', yoyo: true, repeat: 1 }, 0.45);
    letters.forEach((l, i) => {
      tl.fromTo(l, { x: 0, y: 0, rotation: 0, opacity: 1 }, {
        x: () => FLYING[i].x * F(),
        y: () => FLYING[i].y * F(),
        rotation: FLYING[i].r,
        opacity: FLYING[i].o,
        duration: 0.4,
        ease: 'back.out(2.2)',
      }, 0.48 + i * 0.07);
    });
    tl.to('.hero__hint', { autoAlpha: 0, duration: 0.15 }, 0);

    // Le héros apparaît au chargement.
    gsap.from(['.hero__kicker', '.logo', '.hero__foot > *'], { y: 24, autoAlpha: 0, duration: 0.9, ease: 'power3.out', stagger: 0.08, delay: 0.1 });

    // Les quatre refus se rayent en passant.
    gsap.utils.toArray('.never__t').forEach((t) => {
      gsap.fromTo(t, { '--strike': 0 }, {
        '--strike': 1, ease: 'power1.inOut',
        scrollTrigger: { trigger: t, start: 'top 80%', end: 'top 45%', scrub: true },
      });
    });
    gsap.from('.never__title', { y: 40, autoAlpha: 0, duration: 1, ease: 'power3.out', scrollTrigger: { trigger: '.never', start: 'top 70%' } });

    // Les tampons tombent sur la page.
    gsap.from('.stamp', {
      y: 60, rotation: (i) => [-6, 5, -4][i], autoAlpha: 0, duration: 0.8, ease: 'back.out(1.6)', stagger: 0.12,
      clearProps: 'transform',
      scrollTrigger: { trigger: '.levels__row', start: 'top 80%' },
    });

    // Chaque chapitre : la phrase se tape, puis la démonstration arrive.
    gsap.utils.toArray('.ch').forEach((ch) => {
      const ask = ch.querySelector('[data-type]');
      const text = ask.textContent;
      ask.style.minHeight = ask.offsetHeight + 'px';
      ask.textContent = '';
      const tl = gsap.timeline({ scrollTrigger: { trigger: ch, start: 'top 75%', once: true } });
      tl.from(ch.querySelector('.ch__head'), { x: -30, autoAlpha: 0, duration: 0.6, ease: 'power3.out' })
        .add(() => ask.classList.add('is-typing'), 0.1)
        .to({ n: 0 }, {
          n: text.length, duration: Math.min(1.6, text.length * 0.035), ease: 'none',
          onUpdate() { ask.textContent = text.slice(0, Math.round(this.targets()[0].n)); },
          onComplete() { ask.classList.remove('is-typing'); ask.style.minHeight = ''; },
        }, 0.15)
        .from(ch.querySelectorAll('.ch__body p + p'), { y: 16, autoAlpha: 0, duration: 0.5, ease: 'power2.out' }, '<0.3')
        .from(ch.querySelector('.ch__demo'), { y: 40, autoAlpha: 0, duration: 0.7, ease: 'power3.out' }, '<');
    });

    // La fenêtre se redresse en montant.
    gsap.fromTo('.window', { rotation: -2.5, y: 80 }, {
      rotation: 0, y: 0, ease: 'none',
      scrollTrigger: { trigger: '.window', start: 'top bottom', end: 'top 30%', scrub: true },
    });
  });

  // Les polices changent la taille du logotype : on recalcule une fois chargées.
  document.fonts?.ready.then(() => ScrollTrigger.refresh());
})();
