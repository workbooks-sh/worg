defmodule Worg.LoadIfTest do
  use ExUnit.Case, async: true

  alias Worg.LoadIf

  describe "filter/2" do
    test "headline with no :LOAD_IF: always loads" do
      src = """
      * Always here
      Content.
      """

      assert LoadIf.filter(src, %{}) == src
    end

    test "matching :LOAD_IF: keeps the headline" do
      src = """
      * Cinematic playbook
      :PROPERTIES:
      :LOAD_IF: concept=cinematic
      :END:
      Cinematography rules.
      """

      assert LoadIf.filter(src, %{"concept" => "cinematic"}) == src
    end

    test "non-matching :LOAD_IF: strips the headline + descendants" do
      src = """
      * Organic playbook
      :PROPERTIES:
      :LOAD_IF: concept=organic
      :END:
      Organic rules.
      ** Subtopic
      More content.
      """

      filtered = LoadIf.filter(src, %{"concept" => "cinematic"})
      refute filtered =~ "Organic playbook"
      refute filtered =~ "Organic rules"
      refute filtered =~ "Subtopic"
    end

    test "sibling headlines are independently evaluated" do
      src = """
      * Cinematic playbook
      :PROPERTIES:
      :LOAD_IF: concept=cinematic
      :END:
      Cinematography content.

      * Organic playbook
      :PROPERTIES:
      :LOAD_IF: concept=organic
      :END:
      Organic content.

      * Always-loaded reference
      Standard tools available regardless of concept.
      """

      filtered = LoadIf.filter(src, %{"concept" => "cinematic"})
      assert filtered =~ "Cinematic playbook"
      assert filtered =~ "Cinematography content"
      refute filtered =~ "Organic playbook"
      refute filtered =~ "Organic content"
      assert filtered =~ "Always-loaded reference"
      assert filtered =~ "Standard tools available"
    end

    test "deeper sibling headline reawakens after stripped subtree ends" do
      src = """
      * Stripped
      :PROPERTIES:
      :LOAD_IF: x=y
      :END:
      gone

      ** also stripped
      gone too

      * Kept
      stays
      """

      filtered = LoadIf.filter(src, %{"x" => "z"})
      refute filtered =~ "Stripped"
      refute filtered =~ "also stripped"
      refute filtered =~ "gone"
      assert filtered =~ "* Kept"
      assert filtered =~ "stays"
    end

    test "case-insensitive on :LOAD_IF: property name" do
      src = """
      * H
      :PROPERTIES:
      :load_if: x=y
      :END:
      """

      assert LoadIf.filter(src, %{"x" => "y"}) =~ "* H"
      refute LoadIf.filter(src, %{"x" => "no"}) =~ "* H"
    end

    test "preamble before first headline survives unchanged" do
      src = """
      #+TITLE: Doc
      #+TODO: TODO | DONE

      Preamble paragraph.

      * Stripped
      :PROPERTIES:
      :LOAD_IF: never=true
      :END:
      """

      filtered = LoadIf.filter(src, %{})
      assert filtered =~ "#+TITLE: Doc"
      assert filtered =~ "#+TODO: TODO | DONE"
      assert filtered =~ "Preamble paragraph"
      refute filtered =~ "Stripped"
    end

    test "empty vars match nothing → all guarded subtrees strip" do
      src = """
      * A
      :PROPERTIES:
      :LOAD_IF: concept=x
      :END:
      * B
      :PROPERTIES:
      :LOAD_IF: concept=y
      :END:
      * C
      Unguarded.
      """

      filtered = LoadIf.filter(src, %{})
      refute filtered =~ "* A"
      refute filtered =~ "* B"
      assert filtered =~ "* C"
    end

    test "malformed :LOAD_IF: expression fails safe (strips)" do
      src = """
      * Headline
      :PROPERTIES:
      :LOAD_IF: noequalsign
      :END:
      """

      # No `=` in the expr → can't evaluate → fail safe to "don't load".
      refute LoadIf.filter(src, %{}) =~ "Headline"
    end
  end
end
